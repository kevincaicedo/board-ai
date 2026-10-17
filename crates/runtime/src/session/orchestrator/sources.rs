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
    BriefTopic, CachedOverview, Citation, DragOver, KeptSource, Project, ProjectId, SourceEffect,
    SourceFailure, SourceId, SourceInfo, SourceInput, SourceKind,
};
use futures_util::FutureExt;

use super::Orchestrator;
use crate::limits::SESSION_START_TIMEOUT_MS;
use crate::session::inbox::Envelope;
use crate::sources::{
    CachedBrief, Found, Stamp, ValidSource, Written, check_citations, expanded_path, resolve,
    stamp, validate,
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

    /// A readable document goes to the first reader that takes its format (none until T-031:
    /// it then waits in the tray).
    fn read_document(&mut self, source: SourceId) {
        let (Some(SourceKind::Document { format }), Some(path)) = (
            self.desk.kind(source),
            self.desk.path(source).map(std::path::Path::to_path_buf),
        ) else {
            return;
        };
        let Some(reader) = self
            .readers
            .iter()
            .find(|reader| reader.reads(format))
            .cloned()
        else {
            return;
        };
        let cancel = crate::cancel::CancelToken::new();
        let events = reader.read(path, cancel.clone());
        let name = reader.name().to_owned();
        if !self
            .work
            .run_reader(source, name, events, cancel, &self.inbox)
        {
            tracing::warn!("a document waits: as many readers run as may");
        }
    }

    /// The repo tools read the code folders and every readable source. When that turns the
    /// repo reads on or off (the first code source shared with no code folder open, or the
    /// last one let go), the core is told, for D-232's retry (D-244).
    fn refresh_roots(&mut self) {
        match self.desk.roots() {
            Ok(roots) => {
                let before = self.work.repo_reads();
                self.work.set_tools(RepoTools::new(roots));
                let reads = self.work.repo_reads();
                if reads != before {
                    self.answer(SourceInput::RepoReads { reads });
                }
            }
            Err(error) => {
                tracing::warn!(detail = %error.message, "the sources could not be opened")
            }
        }
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
            } => {
                let mut roots = self.desk.code_roots().to_vec();
                for folder in self.desk_folders() {
                    if !roots.contains(&folder) {
                        roots.push(folder);
                    }
                }
                let home = self.desk.home().to_path_buf();
                let job = async move {
                    let found =
                        tokio::task::spawn_blocking(move || resolve(&name, kind, &roots, &home))
                            .await
                            .unwrap_or_default();
                    Some(Envelope::Source(SourceMessage::Found { request, found }))
                };
                if !self.work.spawn_source_job(job.boxed(), &self.inbox) {
                    self.answer(SourceInput::Resolved {
                        request,
                        candidates: Vec::new(),
                        searched: 0,
                    });
                }
            }
            SourceEffect::Share { source } => {
                if self.desk.share(source) {
                    self.refresh_roots();
                    self.read_document(source);
                }
            }
            SourceEffect::Discard { sources } => {
                for source in &sources {
                    self.work.stop_reader(*source);
                }
                self.desk.discard(&sources);
                self.refresh_roots();
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
        if !self.work.spawn_source_job(job.boxed(), &self.inbox) {
            self.answer(SourceInput::Failed {
                failure: SourceFailure::ProjectUnreadable,
                detail: "too many source jobs are running".to_owned(),
            });
        }
    }

    fn keep(&mut self, source: SourceId, project: Project) {
        let (Some(path), Some(kind)) = (self.desk.written_path(source), self.desk.kind(source))
        else {
            return;
        };
        self.desk.set_kept(source, true);
        self.desk.wrote(Written::Kept {
            project: project.id.clone(),
            path: path.clone(),
        });
        let briefs = self.stored_briefs(source);
        for brief in &briefs {
            self.desk.wrote(Written::Brief {
                project: project.id.clone(),
                source: brief.source.clone(),
                topic: brief.topic.clone(),
            });
        }
        let shelf = Arc::clone(&self.shelf);
        let job = async move {
            let line = KeptLine { kind, path };
            if let Err(error) = shelf.keep(project.clone(), line).await {
                let failed = SourceInput::Failed {
                    failure: SourceFailure::NotKept,
                    detail: error.0,
                };
                return Some(Envelope::Input(Input::Source { source: failed }));
            }
            for brief in briefs {
                if let Err(error) = shelf.save_brief(project.id.clone(), brief).await {
                    tracing::warn!(detail = %error.0, "a kept source's brief wasn't saved");
                }
            }
            None
        };
        let _ = self.work.spawn_source_job(job.boxed(), &self.inbox);
    }

    /// The briefs cached for a source, as the project's folder keeps them.
    fn stored_briefs(&self, source: SourceId) -> Vec<StoredBrief> {
        let (Some(path), Some(written)) = (self.desk.path(source), self.desk.written_path(source))
        else {
            return Vec::new();
        };
        self.desk
            .cached_for(path)
            .into_iter()
            .map(|(topic, brief)| StoredBrief {
                source: written.clone(),
                topic,
                stamp: brief.stamp.as_str().to_owned(),
                written: brief.written,
                markdown: brief.markdown,
            })
            .collect()
    }

    fn unkeep(&mut self, source: SourceId, project: ProjectId) {
        let Some(path) = self.desk.written_path(source) else {
            return;
        };
        self.desk.set_kept(source, false);
        let shelf = Arc::clone(&self.shelf);
        let job = async move {
            if let Err(error) = shelf.unkeep(project, path).await {
                let failed = SourceInput::Failed {
                    failure: SourceFailure::NotKept,
                    detail: error.0,
                };
                return Some(Envelope::Input(Input::Source { source: failed }));
            }
            None
        };
        let _ = self.work.spawn_source_job(job.boxed(), &self.inbox);
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
        if !self.work.spawn_source_job(job.boxed(), &self.inbox) {
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
        let detail = if self.desk.has_no_reach(source) {
            "a file loose in the home folder is read by BoardAI alone, not by the harness"
        } else {
            "the source isn't shared any more"
        };
        self.own_answers.push_back(Input::Harness {
            task,
            event: HarnessEvent::Failed {
                reason: HarnessFailure::NotInstalled,
                detail: detail.to_owned(),
            },
        });
    }

    fn stamp_brief(&mut self, source: SourceId, topic: String, markdown: String) {
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
        let _ = self.work.spawn_source_job(job.boxed(), &self.inbox);
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
        if !self.work.spawn_source_job(job.boxed(), &self.inbox) {
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

    fn on_stamped(&mut self, source: SourceId, topic: String, markdown: String, stamp: Stamp) {
        let (Some(path), Some(written_path)) = (
            self.desk.path(source).map(std::path::Path::to_path_buf),
            self.desk.written_path(source),
        ) else {
            return;
        };
        let written = self.wall.now().date();
        let cached = CachedBrief {
            markdown: markdown.clone(),
            stamp: stamp.clone(),
            written: written.clone(),
        };
        self.desk.cache(path, topic.clone(), cached);
        let Some(project) = self
            .desk
            .project()
            .filter(|_| self.desk.is_kept(source))
            .map(|project| project.id.clone())
        else {
            return;
        };
        self.desk.wrote(Written::Brief {
            project: project.clone(),
            source: written_path.clone(),
            topic: topic.clone(),
        });
        let brief = StoredBrief {
            source: written_path,
            topic,
            stamp: stamp.as_str().to_owned(),
            written,
            markdown,
        };
        let shelf = Arc::clone(&self.shelf);
        let job = async move {
            if let Err(error) = shelf.save_brief(project, brief).await {
                tracing::warn!(detail = %error.0, "a brief wasn't saved in the project");
            }
            None
        };
        let _ = self.work.spawn_source_job(job.boxed(), &self.inbox);
    }

    /// "Don't remember this session" (I10): what it kept in a project and the briefs it saved
    /// there are taken back.
    pub(super) fn forget_written(&mut self) {
        let written = self.desk.take_written();
        if written.is_empty() {
            return;
        }
        let shelf = Arc::clone(&self.shelf);
        let job = async move {
            for item in written {
                let undone = match item {
                    Written::Kept { project, path } => shelf.unkeep(project, path).await,
                    Written::Brief {
                        project,
                        source,
                        topic,
                    } => shelf.delete_brief(project, source, topic).await,
                };
                if let Err(error) = undone {
                    tracing::warn!(detail = %error.0, "a source write of a forgotten session stayed");
                }
            }
            None
        };
        if !self.work.spawn_source_job(job.boxed(), &self.inbox) {
            tracing::error!("a forgotten session's source writes could not be undone now");
        }
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
