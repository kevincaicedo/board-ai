//! Coding harnesses (LLD §3.2, §3.5, D-010): Claude Code in headless mode and pi in RPC mode,
//! each one subprocess per task, mapped to the core's provider-neutral [`HarnessEvent`]s.
//!
//! [`HarnessPool`] enforces the limits around any harness; the adapters own only their wire
//! protocol. Every stream ends with exactly one `Finished` or `Failed`, and dropping a stream
//! kills its process.

mod claude;
mod discovery;
mod git;
mod lines;
mod pi;
mod pool;
mod process;
mod text;

use boardai_core::harness::{HarnessEvent, HarnessTask};
use futures_util::stream::BoxStream;

use crate::cancel::CancelToken;

pub use claude::{ClaudeCodeConfig, ClaudeCodeHarness};
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

pub trait CodingHarness: Send + Sync {
    /// Runs one task. `cancel` asks it to stop (SIGINT, then a kill after the grace period).
    fn run(&self, task: HarnessTask, cancel: CancelToken) -> HarnessStream;
}
