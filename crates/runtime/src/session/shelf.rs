//! Where a project keeps its sources and their briefs (D-184, D-185): the vault, through the
//! store (the only writer, law 3), or memory for the run when the vault can't be used. The
//! orchestrator calls it from jobs it owns, never on its own loop.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use boardai_core::sources::{Project, ProjectId};
use futures_util::future::BoxFuture;

use crate::store::{KeptLine, Store, StoredBrief};

/// Why the shelf didn't do something; said in plain words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShelfError(pub String);

/// A project's sources, kept.
pub trait SourceShelf: Send + Sync {
    /// The project the latest session worked on, for the next one to start with.
    fn last_project(&self) -> BoxFuture<'static, Option<Project>>;
    /// The session works on `project` now (written into its digest).
    fn use_project(&self, project: Project) -> BoxFuture<'static, Result<(), ShelfError>>;
    fn kept(&self, project: ProjectId) -> BoxFuture<'static, Result<Vec<KeptLine>, ShelfError>>;
    fn keep(&self, project: Project, line: KeptLine) -> BoxFuture<'static, Result<(), ShelfError>>;
    fn unkeep(
        &self,
        project: ProjectId,
        path: String,
    ) -> BoxFuture<'static, Result<(), ShelfError>>;
    fn save_brief(
        &self,
        project: ProjectId,
        brief: StoredBrief,
    ) -> BoxFuture<'static, Result<(), ShelfError>>;
    fn briefs(
        &self,
        project: ProjectId,
        source: String,
    ) -> BoxFuture<'static, Result<Vec<StoredBrief>, ShelfError>>;
    fn delete_brief(
        &self,
        project: ProjectId,
        source: String,
        topic: String,
    ) -> BoxFuture<'static, Result<(), ShelfError>>;
}

/// The vault's shelf.
#[derive(Clone, Debug)]
pub struct VaultShelf {
    store: Store,
}

impl VaultShelf {
    #[must_use]
    pub fn new(store: Store) -> Self {
        Self { store }
    }
}

fn shelf_error(error: &crate::store::StoreError) -> ShelfError {
    ShelfError(error.to_string())
}

impl SourceShelf for VaultShelf {
    fn last_project(&self) -> BoxFuture<'static, Option<Project>> {
        let store = self.store.clone();
        Box::pin(async move {
            store.last_project().await.unwrap_or_else(|error| {
                tracing::warn!(%error, "the last session's project could not be read");
                None
            })
        })
    }

    fn use_project(&self, project: Project) -> BoxFuture<'static, Result<(), ShelfError>> {
        let store = self.store.clone();
        Box::pin(async move {
            store
                .set_session_project(Some(project))
                .await
                .map_err(|error| shelf_error(&error))
        })
    }

    fn kept(&self, project: ProjectId) -> BoxFuture<'static, Result<Vec<KeptLine>, ShelfError>> {
        let store = self.store.clone();
        Box::pin(async move {
            store
                .project_sources(&project)
                .await
                .map_err(|error| shelf_error(&error))
        })
    }

    fn keep(&self, project: Project, line: KeptLine) -> BoxFuture<'static, Result<(), ShelfError>> {
        let store = self.store.clone();
        Box::pin(async move {
            store
                .keep_source(&project, line)
                .await
                .map_err(|error| shelf_error(&error))
        })
    }

    fn unkeep(
        &self,
        project: ProjectId,
        path: String,
    ) -> BoxFuture<'static, Result<(), ShelfError>> {
        let store = self.store.clone();
        Box::pin(async move {
            store
                .unkeep_source(&project, &path)
                .await
                .map_err(|error| shelf_error(&error))
        })
    }

    fn save_brief(
        &self,
        project: ProjectId,
        brief: StoredBrief,
    ) -> BoxFuture<'static, Result<(), ShelfError>> {
        let store = self.store.clone();
        Box::pin(async move {
            store
                .save_brief(&project, brief)
                .await
                .map_err(|error| shelf_error(&error))
        })
    }

    fn briefs(
        &self,
        project: ProjectId,
        source: String,
    ) -> BoxFuture<'static, Result<Vec<StoredBrief>, ShelfError>> {
        let store = self.store.clone();
        Box::pin(async move {
            store
                .project_briefs(&project, &source)
                .await
                .map_err(|error| shelf_error(&error))
        })
    }

    fn delete_brief(
        &self,
        project: ProjectId,
        source: String,
        topic: String,
    ) -> BoxFuture<'static, Result<(), ShelfError>> {
        let store = self.store.clone();
        Box::pin(async move {
            store
                .delete_brief(&project, &source, &topic)
                .await
                .map_err(|error| shelf_error(&error))
        })
    }
}

#[derive(Debug, Default)]
struct Shelved {
    last: Option<Project>,
    kept: BTreeMap<ProjectId, Vec<KeptLine>>,
    briefs: BTreeMap<(ProjectId, String, String), StoredBrief>,
}

/// A shelf kept in memory for the run: the text dev loop, tests, and the app when the vault
/// can't be used.
#[derive(Clone, Debug, Default)]
pub struct MemoryShelf {
    shelved: Arc<Mutex<Shelved>>,
}

impl MemoryShelf {
    fn with<T>(&self, work: impl FnOnce(&mut Shelved) -> T) -> T {
        let mut guard = self.shelved.lock().unwrap_or_else(PoisonError::into_inner);
        work(&mut guard)
    }
}

impl SourceShelf for MemoryShelf {
    fn last_project(&self) -> BoxFuture<'static, Option<Project>> {
        let last = self.with(|shelved| shelved.last.clone());
        Box::pin(std::future::ready(last))
    }

    fn use_project(&self, project: Project) -> BoxFuture<'static, Result<(), ShelfError>> {
        self.with(|shelved| shelved.last = Some(project));
        Box::pin(std::future::ready(Ok(())))
    }

    fn kept(&self, project: ProjectId) -> BoxFuture<'static, Result<Vec<KeptLine>, ShelfError>> {
        let kept = self.with(|shelved| shelved.kept.get(&project).cloned().unwrap_or_default());
        Box::pin(std::future::ready(Ok(kept)))
    }

    fn keep(&self, project: Project, line: KeptLine) -> BoxFuture<'static, Result<(), ShelfError>> {
        self.with(|shelved| {
            let kept = shelved.kept.entry(project.id).or_default();
            if !kept.iter().any(|known| known.path == line.path) {
                kept.push(line);
            }
        });
        Box::pin(std::future::ready(Ok(())))
    }

    fn unkeep(
        &self,
        project: ProjectId,
        path: String,
    ) -> BoxFuture<'static, Result<(), ShelfError>> {
        self.with(|shelved| {
            if let Some(kept) = shelved.kept.get_mut(&project) {
                kept.retain(|known| known.path != path);
            }
        });
        Box::pin(std::future::ready(Ok(())))
    }

    fn save_brief(
        &self,
        project: ProjectId,
        brief: StoredBrief,
    ) -> BoxFuture<'static, Result<(), ShelfError>> {
        self.with(|shelved| {
            let key = (project, brief.source.clone(), brief.topic.clone());
            shelved.briefs.insert(key, brief);
        });
        Box::pin(std::future::ready(Ok(())))
    }

    fn briefs(
        &self,
        project: ProjectId,
        source: String,
    ) -> BoxFuture<'static, Result<Vec<StoredBrief>, ShelfError>> {
        let found = self.with(|shelved| {
            shelved
                .briefs
                .iter()
                .filter(|((known, path, _), _)| *known == project && *path == source)
                .map(|(_, brief)| brief.clone())
                .collect()
        });
        Box::pin(std::future::ready(Ok(found)))
    }

    fn delete_brief(
        &self,
        project: ProjectId,
        source: String,
        topic: String,
    ) -> BoxFuture<'static, Result<(), ShelfError>> {
        self.with(|shelved| shelved.briefs.remove(&(project, source, topic)));
        Box::pin(std::future::ready(Ok(())))
    }
}
