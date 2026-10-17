//! Coding harnesses (LLD §3.2, §3.5, D-010): Claude Code in headless mode and pi in RPC mode,
//! each one subprocess per task, mapped to the core's provider-neutral [`HarnessEvent`]s.
//!
//! [`HarnessPool`] enforces the limits around any harness; the adapters own only their wire
//! protocol. Every stream ends with exactly one `Finished` or `Failed`, and dropping a stream
//! kills its process.

mod claude;
mod confine;
mod discovery;
mod git;
mod lines;
mod pi;
mod pool;
mod process;
mod sign_in;
mod text;

use std::path::{Path, PathBuf};

use boardai_core::harness::{HarnessEvent, HarnessTask};
use boardai_core::sources::SourceKind;
use futures_util::stream::BoxStream;

use crate::cancel::CancelToken;

pub use claude::{ClaudeCodeConfig, ClaudeCodeHarness, DEFAULT_RESEARCH_MODEL};
pub use discovery::{HarnessDiscovery, discover, discover_from_environment};
pub use git::{BRANCH_PREFIX, is_valid_branch};
pub use pi::{PiConfig, PiHarness};
pub use pool::{HarnessPool, HarnessTimeouts};
pub use sign_in::ClaudeSignIn;

/// The command lines, exposed so tests and the dev overlay can show what runs.
pub mod command_line {
    pub use super::claude::arguments as claude_code;
    pub use super::pi::arguments as pi;
}

pub type HarnessStream = BoxStream<'static, HarnessEvent>;

/// What a task's answer must be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    /// A short answer that can be spoken, citing `path:line`.
    Spoken,
    /// A research brief in the shape the task gives (D-185), parsed by the core, on the
    /// research model.
    Brief,
}

/// Where a task runs and how it answers: the default folder (spoken research, reviews,
/// changes) or a shared source's folder (a brief, D-184, D-185).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    pub folder: PathBuf,
    /// Where a read-only task (research, a review, a brief) may read, canonical: the folder's
    /// enclosing repository, or the folder itself (D-226, D-243). A change works on its own
    /// branch, asked first (I9), and isn't held to it.
    pub reach: PathBuf,
    pub answer: Answer,
}

/// The folder a task runs in when it names none (the first code folder), with its reach.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DefaultFolder {
    pub folder: PathBuf,
    pub reach: PathBuf,
}

impl DefaultFolder {
    /// `folder` and its reach: its enclosing git repository unless that is too broad, else the
    /// folder itself (`sources::reach`, D-243). It looks for `.git` on the disk, once, where the
    /// harness is set up.
    #[must_use]
    pub fn open(folder: PathBuf, home: &Path) -> Self {
        let canonical = std::fs::canonicalize(&folder).unwrap_or_else(|_| folder.clone());
        let reach =
            crate::sources::reach(&canonical, SourceKind::CodeFolder, home).unwrap_or(canonical);
        Self { folder, reach }
    }

    /// A spoken task's workspace here.
    #[must_use]
    pub fn spoken(&self) -> Workspace {
        Workspace {
            folder: self.folder.clone(),
            reach: self.reach.clone(),
            answer: Answer::Spoken,
        }
    }
}

pub trait CodingHarness: Send + Sync {
    /// Runs one task in the configured repo, answering to be spoken. `cancel` asks it to stop
    /// (SIGINT, then a kill after the grace period). With no repo configured it fails at once.
    fn run(&self, task: HarnessTask, cancel: CancelToken) -> HarnessStream;

    /// Runs one task in `workspace.folder`, answering as `workspace.answer` asks.
    fn run_in(&self, task: HarnessTask, workspace: Workspace, cancel: CancelToken)
    -> HarnessStream;
}
