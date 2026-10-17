//! Sources in the orchestrator (D-184, D-185): the core's source effects carried out, the
//! app's drops and drags, and the answers of the jobs it runs for them.
//!
//! - Paths stay in the [`SourceDesk`]; the core gets ids, titles and `~/…` paths.
//! - Slow work (a resolve walk, a project's load, a freshness stamp, reading cited lines, the
//!   shelf's writes) runs as jobs in the session's `Work`, so closing the session stops them,
//!   and answers through the inbox: a core input, or a [`SourceMessage`] when the desk must
//!   change first.
//! - The repo tools follow what is readable: the code folders plus every shared source.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use boardai_core::harness::{HarnessEvent, HarnessFailure, HarnessKind, HarnessTask};
use boardai_core::ids::TaskId;
use boardai_core::input::Input;
use boardai_core::limits::PROJECT_SOURCES_MAX;
use boardai_core::sources::brief::parse;
use boardai_core::sources::{
    BriefTopic, CachedOverview, Citation, DragOver, KeptSource, NameKind, Project, ProjectId,
    SourceEffect, SourceFailure, SourceId, SourceInfo, SourceInput, SourceKind,
};
use futures_util::FutureExt;

use super::Orchestrator;
use crate::limits::SESSION_START_TIMEOUT_MS;
use crate::session::inbox::Envelope;
use crate::sources::{
    CachedBrief, Found, Stamp, ValidSource, check_citations, expanded_path, resolve, stamp,
    validate,
};
use crate::store::{KeptLine, StoredBrief};
use crate::tools::RepoTools;

/// What the app and the orchestrator's own jobs send about sources.
#[derive(Debug)]
pub(crate) enum SourceMessage {
    /// From the app: a path dropped or chosen, already validated. `keep`: the right half.
    Shared { source: ValidSource, keep: bool },
    /// From the app: a drag over the window; `None` once it left or dropped.
    Dragging { drag: Option<DragOver> },
    /// From the app: nothing dropped or chosen passed the checks; `detail` says why.
    Refused { detail: String },
    /// A resolve walk's matches.
    Found { request: u32, found: Found },
    /// A project's kept sources, checked, with their fresh cached briefs.
    Loaded {
        project: Project,
        kept: Vec<Loaded>,
        left_out: u32,
    },
    /// A cached brief's stamp no longer matches the code: research instead.
    Stale {
        task: TaskId,
        source: SourceId,
        topic: String,
        prompt: String,
    },
    /// A brief to cache, stamped.
    Stamped {
        source: SourceId,
        topic: String,
        markdown: String,
        stamp: Stamp,
    },
    /// A document reader's digest, to cache (D-259).
    Digested { source: SourceId, markdown: String },
    /// A cached digest checked against its file: `None` when it is stale.
    DigestChecked {
        source: SourceId,
        markdown: Option<String>,
    },
}

/// One kept source as the load found it.
#[derive(Debug)]
pub(crate) struct Loaded {
    line: KeptLine,
    valid: Option<ValidSource>,
    fresh: Vec<StoredBrief>,
}

impl Orchestrator {
    /// A new session: nothing shared, the repo tools on the code folders only, and the core
    /// told which harness reads code and which project the last session worked on.
    pub(super) async fn start_sources(&mut self) {
        self.desk.start_session();
        self.waiting_readers.clear();
        self.refresh_roots();
        let bound = Duration::from_millis(SESSION_START_TIMEOUT_MS);
        let project = tokio::time::timeout(bound, self.shelf.last_project())
            .await
            .unwrap_or_else(|_elapsed| {
                tracing::warn!("the last session's project wasn't read in time");
                None
            });
        let opened = SourceInput::Opened {
            harness: self.work.has_harness().then(|| {
                self.desk
                    .harness()
                    .unwrap_or("the coding harness")
                    .to_owned()
            }),
            project,
        };
        self.step(Input::Source { source: opened }).await;
    }

    /// The repo tools read the code folders and every readable source. When that turns the
    /// repo reads on or off (the first code source shared with no code folder open, or the
    /// last one let go), the core is told, for D-232's retry (D-244).
    fn refresh_roots(&mut self) {
        match self.desk.roots() {
            Ok(roots) => {
                self.work.set_tools(RepoTools::new(roots));
                if let Some(reads) = self.desk.changed_reads() {
                    self.answer(SourceInput::RepoReads { reads });
                }
            }
            Err(error) => {
                tracing::warn!(detail = %error.message, "the sources could not be opened")
            }
        }
    }

    /// Sources the desk forgot: their readers stop, they leave the readers' line, and the repo
    /// tools stop reading them.
    pub(super) fn let_go(&mut self, gone: &[SourceId]) {
        if gone.is_empty() {
            return;
        }
        for source in gone {
            self.work.stop_reader(*source);
        }
        self.stop_waiting(gone);
        self.refresh_roots();
    }

    fn answer(&mut self, source: SourceInput) {
        self.own_answers.push_back(Input::Source { source });
    }

    pub(super) fn apply_source(&mut self, effect: SourceEffect) {
        match effect {
            SourceEffect::Resolve {
                request,
                name,
                kind,
            } => self.resolve_name(request, name, kind),
            SourceEffect::Share { source } => {
                if self.desk.share(source) {
                    self.refresh_roots();
                    self.read_document(source);
                }
            }
            SourceEffect::Discard { sources } => {
                let gone = self.desk.discard(&sources);
                self.let_go(&gone);
            }
            SourceEffect::SetProject { project } => self.load_project(project),
            SourceEffect::Keep { source, project } => self.keep(source, project),
            SourceEffect::Unkeep { source, project } => self.unkeep(source, project),
            SourceEffect::Brief {
                task,
                source,
                topic,
                prompt,
            } => self.brief(task, source, &topic, prompt),
            SourceEffect::CacheBrief {
                source,
                topic,
                markdown,
            } => self.stamp_brief(source, topic.key(), markdown),
            SourceEffect::CheckCitations {
                task,
                source,
                citations,
            } => self.check(task, source, citations),
            SourceEffect::ReadDocument {
                request,
                source,
                first_page,
                last_page,
            } => self.read_pages(request, source, first_page, last_page),
        }
    }

    /// A spoken name: a bounded walk of the code folders and readable folders, as a job.
    fn resolve_name(&mut self, request: u32, name: String, kind: NameKind) {
        let mut roots = self.desk.code_roots().to_vec();
        for folder in self.desk_folders() {
            if !roots.contains(&folder) {
                roots.push(folder);
            }
        }
        let home = self.desk.home().to_path_buf();
        let job = async move {
            let found = tokio::task::spawn_blocking(move || resolve(&name, kind, &roots, &home))
                .await
                .unwrap_or_default();
            Some(Envelope::Source(SourceMessage::Found { request, found }))
        };
        let nothing = SourceInput::Resolved {
            request,
            candidates: Vec::new(),
            searched: 0,
        };
        let timed_out = Envelope::Input(Input::Source {
            source: nothing.clone(),
        });
        if !self
            .work
            .spawn_source_job(job.boxed(), Some(timed_out), &self.inbox)
        {
            self.answer(nothing);
        }
    }

    /// The folders of readable sources, for the resolver.
    fn desk_folders(&self) -> Vec<PathBuf> {
        self.desk
            .roots()
            .map(|roots| {
                roots
                    .folders()
                    .filter(|path| path.is_dir())
                    .map(std::path::Path::to_path_buf)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn load_project(&mut self, project: Project) {
        self.desk.set_project(project.clone());
        let shelf = Arc::clone(&self.shelf);
        let home = self.desk.home().to_path_buf();
        let job = async move {
            if let Err(error) = shelf.use_project(project.clone()).await {
                tracing::warn!(detail = %error.0, "the session's project wasn't recorded");
            }
            let lines = match shelf.kept(project.id.clone()).await {
                Ok(lines) => lines,
                Err(error) => {
                    let failed = SourceInput::Failed {
                        failure: SourceFailure::ProjectUnreadable,
                        detail: error.0,
                    };
                    return Some(Envelope::Input(Input::Source { source: failed }));
                }
            };
            let left_out =
                u32::try_from(lines.len().saturating_sub(PROJECT_SOURCES_MAX)).unwrap_or(u32::MAX);
            let mut kept = Vec::new();
            for line in lines.into_iter().take(PROJECT_SOURCES_MAX) {
                kept.push(load_one(&*shelf, &project.id, line, &home).await);
            }
            Some(Envelope::Source(SourceMessage::Loaded {
                project,
                kept,
                left_out,
            }))
        };
        let timed_out = Envelope::Input(Input::Source {
            source: SourceInput::Failed {
                failure: SourceFailure::ProjectUnreadable,
                detail: "its sources weren't read in time".to_owned(),
            },
        });
        if !self
            .work
            .spawn_source_job(job.boxed(), Some(timed_out), &self.inbox)
        {
            self.answer(SourceInput::Failed {
                failure: SourceFailure::ProjectUnreadable,
                detail: "too many source jobs are running".to_owned(),
            });
        }
    }

    /// A brief: the cache's when its stamp still matches the code, else the harness.
    fn brief(&mut self, task: TaskId, source: SourceId, topic: &BriefTopic, prompt: String) {
        let (Some(path), Some(_)) = (
            self.desk.path(source).map(std::path::Path::to_path_buf),
            self.desk.workspace(source),
        ) else {
            self.no_workspace(task, source);
            return;
        };
        let key = topic.key();
        let Some(cached) = self.desk.cached(&path, &key).cloned() else {
            self.research(task, source, prompt);
            return;
        };
        let fallback = prompt.clone();
        // A stamp not taken in time can't vouch for the cached brief: research instead.
        let timed_out = Envelope::Source(SourceMessage::Stale {
            task,
            source,
            topic: key.clone(),
            prompt: prompt.clone(),
        });
        let job = async move {
            let now = tokio::task::spawn_blocking(move || stamp(&path)).await.ok();
            if now.as_ref() == Some(&cached.stamp) {
                let source = SourceInput::BriefCached {
                    task,
                    markdown: cached.markdown,
                };
                return Some(Envelope::Input(Input::Source { source }));
            }
            Some(Envelope::Source(SourceMessage::Stale {
                task,
                source,
                topic: key,
                prompt,
            }))
        };
        if !self
            .work
            .spawn_source_job(job.boxed(), Some(timed_out), &self.inbox)
        {
            self.research(task, source, fallback);
        }
    }

    /// Starts the harness on a brief request in the source's workspace.
    fn research(&mut self, task: TaskId, source: SourceId, prompt: String) {
        let Some(workspace) = self.desk.workspace(source) else {
            self.no_workspace(task, source);
            return;
        };
        let spec = HarnessTask {
            kind: HarnessKind::Research,
            brief: prompt,
        };
        match self.work.run_harness_in(task, spec, workspace, &self.inbox) {
            Some(failed) => self.own_answers.push_back(failed),
            None => self.answer(SourceInput::BriefStarted { task }),
        }
    }

    /// A brief that can't run: the source is gone, or nothing around it may be read (D-226).
    fn no_workspace(&mut self, task: TaskId, source: SourceId) {
        let detail = match self.desk.kind(source) {
            Some(SourceKind::Document { .. }) => {
                "a document is read by its reader, never researched around it (D-231)"
            }
            Some(SourceKind::CodeFolder | SourceKind::CodeFile)
                if self.desk.has_no_reach(source) =>
            {
                "a file loose in the home folder is read by BoardAI alone, not by the harness"
            }
            Some(SourceKind::CodeFolder | SourceKind::CodeFile) | None => {
                "the source isn't shared any more"
            }
        };
        self.own_answers.push_back(Input::Harness {
            task,
            event: HarnessEvent::Failed {
                reason: HarnessFailure::NotInstalled,
                detail: detail.to_owned(),
            },
        });
    }

    pub(super) fn stamp_brief(&mut self, source: SourceId, topic: String, markdown: String) {
        let Some(path) = self.desk.path(source).map(std::path::Path::to_path_buf) else {
            return;
        };
        let job = async move {
            let stamp = tokio::task::spawn_blocking(move || stamp(&path))
                .await
                .ok()?;
            Some(Envelope::Source(SourceMessage::Stamped {
                source,
                topic,
                markdown,
                stamp,
            }))
        };
        if !self.work.spawn_source_job(job.boxed(), None, &self.inbox) {
            tracing::warn!("a brief wasn't cached: too many source jobs run");
        }
    }

    fn check(&mut self, task: TaskId, source: SourceId, citations: Vec<Citation>) {
        let (Some(workspace), Some(kind), Some(path)) = (
            self.desk.workspace(source),
            self.desk.kind(source),
            self.desk.path(source).map(std::path::Path::to_path_buf),
        ) else {
            self.answer(SourceInput::CitationsChecked {
                task,
                read: Vec::new(),
                unverified: citations,
            });
            return;
        };
        let only_file = match kind {
            SourceKind::CodeFile => path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned()),
            SourceKind::CodeFolder | SourceKind::Document { .. } => None,
        };
        let beyond = self.desk.cited_beyond(source);
        let job_citations = citations.clone();
        let timed_out = Envelope::Input(Input::Source {
            source: SourceInput::CitationsChecked {
                task,
                read: Vec::new(),
                unverified: citations.clone(),
            },
        });
        let job = async move {
            let checked = tokio::task::spawn_blocking(move || {
                check_citations(
                    &workspace.folder,
                    only_file.as_deref(),
                    beyond.as_deref(),
                    &job_citations,
                )
            })
            .await
            .unwrap_or_default();
            let source = SourceInput::CitationsChecked {
                task,
                read: checked.read,
                unverified: checked.unverified,
            };
            Some(Envelope::Input(Input::Source { source }))
        };
        if !self
            .work
            .spawn_source_job(job.boxed(), Some(timed_out), &self.inbox)
        {
            self.answer(SourceInput::CitationsChecked {
                task,
                read: Vec::new(),
                unverified: citations,
            });
        }
    }

    pub(super) async fn on_source_message(&mut self, message: SourceMessage) {
        match message {
            SourceMessage::Shared { source, keep } => {
                let info = self.desk.register(&source);
                self.desk.share(info.id);
                self.refresh_roots();
                self.read_document(info.id);
                let shared = SourceInput::Shared { source: info, keep };
                self.step(Input::Source { source: shared }).await;
            }
            SourceMessage::Dragging { drag } => {
                self.step(Input::Source {
                    source: SourceInput::Dragging { drag },
                })
                .await;
            }
            SourceMessage::Refused { detail } => {
                let failed = SourceInput::Failed {
                    failure: SourceFailure::Refused,
                    detail,
                };
                self.step(Input::Source { source: failed }).await;
            }
            SourceMessage::Found { request, found } => {
                let candidates: Vec<SourceInfo> = found
                    .candidates
                    .iter()
                    .map(|valid| self.desk.register(valid))
                    .collect();
                let resolved = SourceInput::Resolved {
                    request,
                    candidates,
                    searched: found.searched,
                };
                self.step(Input::Source { source: resolved }).await;
            }
            SourceMessage::Loaded {
                project,
                kept,
                left_out,
            } => self.on_loaded(project, kept, left_out).await,
            SourceMessage::Stale {
                task,
                source,
                topic,
                prompt,
            } => {
                if let Some(path) = self.desk.path(source).map(std::path::Path::to_path_buf) {
                    self.desk.evict(&path, &topic);
                }
                self.research(task, source, prompt);
            }
            SourceMessage::Stamped {
                source,
                topic,
                markdown,
                stamp,
            } => self.on_stamped(source, topic, markdown, stamp),
            SourceMessage::Digested { source, markdown } => self.on_digested(source, markdown),
            SourceMessage::DigestChecked { source, markdown } => {
                self.on_digest_checked(source, markdown).await;
            }
        }
    }

    async fn on_loaded(&mut self, project: Project, kept: Vec<Loaded>, left_out: u32) {
        if self
            .desk
            .project()
            .is_none_or(|current| current.id != project.id)
        {
            return;
        }
        let mut sources = Vec::new();
        for loaded in kept {
            let Some(valid) = loaded.valid else {
                let info = self.desk.register_missing(&loaded.line);
                sources.push(KeptSource {
                    source: info,
                    overview: None,
                    missing: true,
                });
                continue;
            };
            let info = self.desk.register(&valid);
            self.desk.share(info.id);
            self.desk.set_kept(info.id, true);
            self.read_document(info.id);
            let mut overview = None;
            for brief in loaded.fresh {
                if brief.topic == BriefTopic::Overview.key() {
                    let parsed = parse(&brief.markdown);
                    overview = Some(CachedOverview {
                        components: parsed
                            .components
                            .iter()
                            .map(|part| (part.id.as_str().to_owned(), part.name.clone()))
                            .collect(),
                        written: brief.written.clone(),
                    });
                }
                let cached = CachedBrief {
                    markdown: brief.markdown,
                    stamp: Stamp::stored(&brief.stamp),
                    written: brief.written,
                };
                self.desk.cache(valid.path.clone(), brief.topic, cached);
            }
            sources.push(KeptSource {
                source: info,
                overview,
                missing: false,
            });
        }
        self.refresh_roots();
        let loaded = SourceInput::ProjectLoaded {
            project,
            kept: sources,
            left_out,
        };
        self.step(Input::Source { source: loaded }).await;
    }
}

/// One kept line, checked like a drop, with the cached briefs whose stamp still matches.
async fn load_one(
    shelf: &dyn crate::session::shelf::SourceShelf,
    project: &ProjectId,
    line: KeptLine,
    home: &std::path::Path,
) -> Loaded {
    let path = expanded_path(&line.path, home);
    let home_folder = home.to_path_buf();
    let valid = tokio::task::spawn_blocking(move || {
        path.and_then(|path| validate(&path, &home_folder).ok())
    })
    .await
    .ok()
    .flatten();
    let Some(valid) = valid else {
        return Loaded {
            line,
            valid: None,
            fresh: Vec::new(),
        };
    };
    let stored = shelf
        .briefs(project.clone(), line.path.clone())
        .await
        .unwrap_or_default();
    let path = valid.path.clone();
    let now = tokio::task::spawn_blocking(move || stamp(&path)).await.ok();
    let fresh = stored
        .into_iter()
        .filter(|brief| now.as_ref().is_some_and(|now| now.as_str() == brief.stamp))
        .collect();
    Loaded {
        line,
        valid: Some(valid),
        fresh,
    }
}
