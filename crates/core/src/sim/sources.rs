//! A scripted runtime for sources (D-184, D-185): the name resolver, the vault's kept sources,
//! the brief cache, the harness writing briefs, and native reads of cited lines.

use std::collections::BTreeMap;

use crate::harness::HarnessEvent;
use crate::ids::TaskId;
use crate::input::Input;
use crate::sources::{
    Citation, CitedLines, KeptSource, ProjectId, SourceEffect, SourceId, SourceInfo, SourceInput,
    SourceKind, name_key,
};

/// How long the scripted runtime takes to answer a resolve, a cache lookup or a read.
pub const SOURCES_LATENCY_MS: u64 = 5;

#[derive(Clone, Debug)]
pub struct FakeSources {
    /// Folders the resolver can find, by title.
    pub folders: Vec<SourceInfo>,
    /// What each project keeps; moved from one simulated session to the next.
    pub vault: BTreeMap<ProjectId, Vec<SourceInfo>>,
    /// Fresh cached briefs, by the source's path and the topic key (the runtime's cache knows
    /// paths, not this session's ids).
    pub cache: BTreeMap<(String, String), String>,
    /// The brief the harness writes for a topic key; `default_brief` otherwise.
    pub briefs: BTreeMap<String, String>,
    pub default_brief: String,
    /// Files a native read finds, with their line count.
    pub readable: BTreeMap<String, u32>,
    /// Files the harness reads before it answers.
    pub touched: Vec<String>,
    pub run_ms: u64,
    /// Every effect, in order.
    pub seen: Vec<SourceEffect>,
    /// Sources readable now: shared, or kept in the loaded project.
    pub readable_sources: Vec<SourceId>,
    /// Every source registered, by id, so keeping one can write it to the vault.
    known: BTreeMap<SourceId, SourceInfo>,
    next_id: u32,
}

impl Default for FakeSources {
    fn default() -> Self {
        Self {
            folders: Vec::new(),
            vault: BTreeMap::new(),
            cache: BTreeMap::new(),
            briefs: BTreeMap::new(),
            default_brief: String::new(),
            readable: BTreeMap::new(),
            touched: Vec::new(),
            run_ms: 3_000,
            seen: Vec::new(),
            readable_sources: Vec::new(),
            known: BTreeMap::new(),
            next_id: 100,
        }
    }
}

/// A source as the runtime would register it.
#[must_use]
pub fn code_folder(id: u32, title: &str, path: &str) -> SourceInfo {
    SourceInfo {
        id: SourceId(id),
        kind: SourceKind::CodeFolder,
        title: title.to_owned(),
        shown_path: path.to_owned(),
        files_estimate: Some(40),
    }
}

impl FakeSources {
    /// The runtime's answers to one effect, each with its delay from now.
    pub fn answer(&mut self, effect: &SourceEffect) -> Vec<(u64, Input)> {
        self.seen.push(effect.clone());
        let source = |source| Input::Source { source };
        match effect {
            SourceEffect::Resolve { request, name, .. } => {
                let key = name_key(name);
                let found: Vec<SourceInfo> = self
                    .folders
                    .iter()
                    .filter(|folder| name_key(&folder.title) == key)
                    .cloned()
                    .collect();
                let candidates = found.iter().map(|folder| self.register(folder)).collect();
                let resolved = SourceInput::Resolved {
                    request: *request,
                    candidates,
                    searched: 12,
                };
                vec![(SOURCES_LATENCY_MS, source(resolved))]
            }
            SourceEffect::Share { source: id } => {
                self.readable_sources.push(*id);
                Vec::new()
            }
            SourceEffect::Discard { sources } => {
                self.readable_sources.retain(|id| !sources.contains(id));
                Vec::new()
            }
            SourceEffect::SetProject { project } => {
                let loaded = self.load(project);
                vec![(SOURCES_LATENCY_MS, source(loaded))]
            }
            SourceEffect::Keep {
                source: id,
                project,
            } => {
                if let Some(info) = self.known.get(id).cloned() {
                    self.keep_in_vault(&project.id, info);
                }
                Vec::new()
            }
            SourceEffect::Unkeep {
                source: id,
                project,
            } => {
                self.unkeep(*id, project);
                Vec::new()
            }
            SourceEffect::Brief {
                task,
                source: id,
                topic,
                ..
            } => self.brief(*task, *id, &topic.key()),
            SourceEffect::CacheBrief {
                source: id,
                topic,
                markdown,
            } => {
                if let Some(info) = self.known.get(id) {
                    self.cache
                        .insert((info.shown_path.clone(), topic.key()), markdown.clone());
                }
                Vec::new()
            }
            SourceEffect::CheckCitations {
                task, citations, ..
            } => vec![(SOURCES_LATENCY_MS, self.check(*task, citations))],
        }
    }

    /// Native reads of cited lines: a file it knows, within its length.
    fn check(&self, task: TaskId, citations: &[Citation]) -> Input {
        let mut read = Vec::new();
        let mut unverified = Vec::new();
        for citation in citations {
            match self.readable.get(&citation.path) {
                Some(count) if citation.last_line <= *count => read.push(CitedLines {
                    path: citation.path.clone(),
                    first_line: citation.first_line,
                    lines: (citation.first_line..=citation.last_line)
                        .map(|line| format!("// {} line {line}", citation.path))
                        .collect(),
                }),
                Some(_) | None => unverified.push(citation.clone()),
            }
        }
        Input::Source {
            source: SourceInput::CitationsChecked {
                task,
                read,
                unverified,
            },
        }
    }

    /// A project's kept sources, registered anew as a new session's runtime would.
    fn load(&mut self, project: &crate::sources::Project) -> SourceInput {
        let stored = self.vault.get(&project.id).cloned().unwrap_or_default();
        let kept: Vec<KeptSource> = stored
            .iter()
            .map(|info| KeptSource {
                source: self.register(info),
                overview: None,
                missing: false,
            })
            .collect();
        self.readable_sources
            .extend(kept.iter().map(|kept| kept.source.id));
        SourceInput::ProjectLoaded {
            project: project.clone(),
            kept,
            left_out: 0,
        }
    }

    fn unkeep(&mut self, id: SourceId, project: &ProjectId) {
        let path = self.known.get(&id).map(|info| info.shown_path.clone());
        if let (Some(kept), Some(path)) = (self.vault.get_mut(project), path) {
            kept.retain(|info| info.shown_path != path);
        }
    }

    /// Records a kept source in the vault, as the store would.
    pub fn keep_in_vault(&mut self, project: &ProjectId, info: SourceInfo) {
        let kept = self.vault.entry(project.clone()).or_default();
        if !kept.iter().any(|known| known.shown_path == info.shown_path) {
            kept.push(info);
        }
    }

    /// A source shared natively (a drop), as the runtime registered it and made it readable.
    pub fn shared(&mut self, info: &SourceInfo) {
        self.known.insert(info.id, info.clone());
        self.readable_sources.push(info.id);
    }

    /// The same path under a new id, as a new session's runtime registers it.
    fn register(&mut self, info: &SourceInfo) -> SourceInfo {
        self.next_id += 1;
        let registered = SourceInfo {
            id: SourceId(self.next_id),
            ..info.clone()
        };
        self.known.insert(registered.id, registered.clone());
        registered
    }

    fn brief(&mut self, task: TaskId, id: SourceId, key: &str) -> Vec<(u64, Input)> {
        let path = self
            .known
            .get(&id)
            .map(|info| info.shown_path.clone())
            .unwrap_or_default();
        if let Some(markdown) = self.cache.get(&(path, key.to_owned())) {
            let cached = SourceInput::BriefCached {
                task,
                markdown: markdown.clone(),
            };
            return vec![(SOURCES_LATENCY_MS, Input::Source { source: cached })];
        }
        let summary = self
            .briefs
            .get(key)
            .cloned()
            .unwrap_or_else(|| self.default_brief.clone());
        let mut inputs = vec![(
            SOURCES_LATENCY_MS,
            Input::Source {
                source: SourceInput::BriefStarted { task },
            },
        )];
        inputs.push((
            SOURCES_LATENCY_MS * 2,
            Input::Harness {
                task,
                event: HarnessEvent::Progress {
                    text: "Reading the arena first.".to_owned(),
                },
            },
        ));
        let count = u64::try_from(self.touched.len()).unwrap_or(0) + 1;
        for (index, path) in (1..).zip(&self.touched) {
            inputs.push((
                self.run_ms * index / count,
                Input::Harness {
                    task,
                    event: HarnessEvent::FileTouched { path: path.clone() },
                },
            ));
        }
        inputs.push((
            self.run_ms,
            Input::Harness {
                task,
                event: HarnessEvent::Finished {
                    summary,
                    citations: Vec::new(),
                },
            },
        ));
        inputs
    }
}
