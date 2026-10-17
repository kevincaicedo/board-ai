//! What a session writes to a project's folder about sources (D-184, D-221, D-259): a kept
//! source's line in `sources.md`, and the briefs and whole digests cached for a kept source.
//!
//! - Every write runs as a source job, after the one asked before it has ended (or was dropped):
//!   they land in the order asked, so "don't remember this session" undoes every write asked
//!   before it and none lands after its undo.
//! - Once the session is forgotten (I10) nothing more of it is written there: a keep, a brief or
//!   a digest that arrives later stays in the session's memory, whatever the core asks.

use std::sync::Arc;

use boardai_core::input::Input;
use boardai_core::sources::{Project, ProjectId, SourceFailure, SourceId, SourceInput};
use futures_util::FutureExt;
use futures_util::future::BoxFuture;
use tokio::sync::oneshot;

use super::Orchestrator;
use crate::session::inbox::Envelope;
use crate::sources::{CachedBrief, Stamp, Written};
use crate::store::{KeptLine, StoredBrief};

impl Orchestrator {
    /// Runs a vault write after the last one asked; false when it couldn't be queued
    /// (`SOURCE_JOBS_MAX`), which drops it.
    fn shelf_write(&mut self, write: BoxFuture<'static, Option<Envelope>>) -> bool {
        let (done, next) = oneshot::channel::<()>();
        let previous = self.shelf_order.replace(next);
        let job = async move {
            // Ended, or dropped with its job: either way the next may go.
            if let Some(previous) = previous {
                let _ = previous.await;
            }
            let answer = write.await;
            drop(done);
            answer
        };
        self.work.spawn_source_job(job.boxed(), None, &self.inbox)
    }

    pub(super) fn keep(&mut self, source: SourceId, project: Project) {
        if self.desk.is_forgotten() {
            tracing::info!("a keep after \"don't remember this session\" isn't written");
            return;
        }
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
        if !self.shelf_write(job.boxed()) {
            tracing::warn!("a kept source wasn't written: too many source jobs run");
        }
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

    pub(super) fn unkeep(&mut self, source: SourceId, project: ProjectId) {
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
        if !self.shelf_write(job.boxed()) {
            tracing::warn!("a source wasn't let go in the project: too many source jobs run");
        }
    }

    /// A brief or a whole digest, stamped: cached for the session, and saved in the project's
    /// folder when its source is kept there and the session isn't forgotten.
    pub(super) fn on_stamped(
        &mut self,
        source: SourceId,
        topic: String,
        markdown: String,
        stamp: Stamp,
    ) {
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
            .filter(|_| self.desk.is_kept(source) && !self.desk.is_forgotten())
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
        if !self.shelf_write(job.boxed()) {
            tracing::warn!("a brief wasn't saved in the project: too many source jobs run");
        }
    }

    /// "Don't remember this session" (I10): what it kept in a project and the briefs it saved
    /// there are taken back, after every write asked before; its keeps are no longer kept, and
    /// nothing more of it is written there.
    pub(super) fn forget_written(&mut self) {
        let written = self.desk.forget_session();
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
        if !self.shelf_write(job.boxed()) {
            tracing::error!("a forgotten session's source writes could not be undone now");
        }
    }
}
