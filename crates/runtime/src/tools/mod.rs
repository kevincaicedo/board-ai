//! The talker's native, read-only repo tools (LLD §3.3, §5, §9): `read_file`, `search_code`
//! and `list_dir`, confined to the opened folders and bounded. They run on the blocking pool so
//! a slow disk never stalls the orchestrator.

mod read;
mod repo_path;
mod search;
mod text;

use std::sync::Arc;

use boardai_core::tools::{ToolCall, ToolFailure, ToolOutput};

pub use repo_path::{RepoPath, RepoRoots, ToolError};

/// The repo tools over a fixed set of opened folders. Cheap to clone.
#[derive(Clone, Debug, Default)]
pub struct RepoTools {
    roots: Arc<RepoRoots>,
}

impl RepoTools {
    #[must_use]
    pub fn new(roots: RepoRoots) -> Self {
        Self {
            roots: Arc::new(roots),
        }
    }

    #[must_use]
    pub fn roots(&self) -> &RepoRoots {
        &self.roots
    }

    /// Runs a repo tool. Every failure is a `ToolOutput::Failed` the talker can read. If this
    /// future is dropped, the blocking work still finishes, bounded by the tool limits.
    pub async fn run(&self, call: &ToolCall) -> ToolOutput {
        let roots = Arc::clone(&self.roots);
        let call = call.clone();
        let result = tokio::task::spawn_blocking(move || run_blocking(&roots, &call)).await;
        match result {
            Ok(Ok(output)) => output,
            Ok(Err(error)) => ToolOutput::Failed {
                kind: error.kind,
                message: error.message,
            },
            Err(join_error) => ToolOutput::Failed {
                kind: ToolFailure::Io,
                message: format!("the tool stopped unexpectedly: {join_error}"),
            },
        }
    }
}

/// The same tools, on the calling thread (tests, and callers already on a blocking thread).
pub fn run_blocking(roots: &RepoRoots, call: &ToolCall) -> Result<ToolOutput, ToolError> {
    match call {
        ToolCall::ReadFile {
            path,
            start_line,
            end_line,
        } => read::read_file(roots, path, *start_line, *end_line),
        ToolCall::SearchCode { query, glob } => search::search_code(roots, query, glob.as_deref()),
        ToolCall::ListDir { path } => search::list_dir(roots, path),
        ToolCall::Research { .. }
        | ToolCall::Review { .. }
        | ToolCall::ProposeChange { .. }
        | ToolCall::Remember { .. }
        | ToolCall::AddTask { .. }
        | ToolCall::Recall { .. }
        | ToolCall::OpenSession { .. } => Err(ToolError::new(
            ToolFailure::InvalidInput,
            "not a repo tool; the orchestrator routes it elsewhere",
        )),
    }
}
