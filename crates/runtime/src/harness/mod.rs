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
mod text;

use std::path::PathBuf;

use boardai_core::harness::{HarnessEvent, HarnessTask};
use futures_util::stream::BoxStream;

use crate::cancel::CancelToken;

pub use claude::{ClaudeCodeConfig, ClaudeCodeHarness, DEFAULT_RESEARCH_MODEL};
pub use discovery::{HarnessDiscovery, discover, discover_from_environment};
pub use git::{BRANCH_PREFIX, is_valid_branch};
pub use pi::{PiConfig, PiHarness};
pub use pool::{HarnessPool, HarnessTimeouts};

/// The command lines, exposed so tests and the dev overlay can show what runs.
pub mod command_line {
    pub use super::claude::arguments as claude_code;
    pub use super::pi::arguments as pi;
}

pub type HarnessStream = BoxStream<'static, HarnessEvent>;

/// What a task's answer must be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// A short answer that can be spoken, citing `path:line`.
    Spoken,
    /// A research brief in the shape the task gives (D-185), parsed by the core, read only
    /// inside `reach` (canonical: the source's enclosing repository, D-226) and on the research
    /// model.
    Brief { reach: PathBuf },
}

/// Where a task runs and how it answers: a shared source's folder and a brief (D-184, D-185).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    pub folder: PathBuf,
    pub answer: Answer,
}

pub trait CodingHarness: Send + Sync {
    /// Runs one task in the configured repo, answering to be spoken. `cancel` asks it to stop
    /// (SIGINT, then a kill after the grace period). With no repo configured it fails at once.
    fn run(&self, task: HarnessTask, cancel: CancelToken) -> HarnessStream;

    /// Runs one task in `workspace.folder`, answering as `workspace.answer` asks.
    fn run_in(&self, task: HarnessTask, workspace: Workspace, cancel: CancelToken)
    -> HarnessStream;
}
