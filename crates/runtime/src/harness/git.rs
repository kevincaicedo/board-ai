//! The git steps before a change task (D-018): refuse a dirty tree, then create the task's
//! branch and switch to it, so the harness's edits land only there.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use boardai_core::harness::{HarnessEvent, HarnessFailure};
use tokio::process::Command;

use crate::limits::{GIT_COMMAND_TIMEOUT_MS, HARNESS_FAILURE_DETAIL_MAX_BYTES};

/// Branches BoardAI creates are always under this prefix.
pub const BRANCH_PREFIX: &str = "boardai/";

/// Longest slug after the prefix.
const SLUG_MAX_BYTES: usize = 64;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum GitError {
    InvalidBranch(String),
    DirtyTree,
    Failed(String),
    Cancelled,
}

impl GitError {
    /// `HarnessFailure` has no "refused" kind yet, so a refused precondition is reported as
    /// `Crashed` with a detail that says what to do (see the report's contract proposal).
    pub(crate) fn into_event(self) -> HarnessEvent {
        let (reason, detail) = match self {
            Self::InvalidBranch(branch) => (
                HarnessFailure::Crashed,
                format!("refused: `{branch}` is not a valid {BRANCH_PREFIX}<slug> branch"),
            ),
            Self::DirtyTree => (
                HarnessFailure::Crashed,
                "refused: the working tree has uncommitted changes; commit or stash them first"
                    .to_owned(),
            ),
            Self::Failed(detail) => (HarnessFailure::Crashed, format!("git failed: {detail}")),
            Self::Cancelled => (HarnessFailure::Cancelled, "cancelled".to_owned()),
        };
        HarnessEvent::Failed { reason, detail }
    }
}

/// `boardai/` followed by 1 to 64 bytes of lowercase letters, digits and single hyphens.
#[must_use]
pub fn is_valid_branch(branch: &str) -> bool {
    let Some(slug) = branch.strip_prefix(BRANCH_PREFIX) else {
        return false;
    };
    !slug.is_empty()
        && slug.len() <= SLUG_MAX_BYTES
        && slug
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !slug.starts_with('-')
        && !slug.ends_with('-')
        && !slug.contains("--")
}

pub(crate) async fn prepare_branch(repo: &Path, branch: &str) -> Result<(), GitError> {
    if !is_valid_branch(branch) {
        return Err(GitError::InvalidBranch(branch.to_owned()));
    }
    // Untracked files count as dirty too: the harness's new files must be its own.
    let status = git(repo, &["status", "--porcelain"]).await?;
    if !status.trim().is_empty() {
        return Err(GitError::DirtyTree);
    }
    git(repo, &["switch", "--create", branch]).await?;
    Ok(())
}

async fn git(repo: &Path, arguments: &[&str]) -> Result<String, GitError> {
    let output = tokio::time::timeout(
        Duration::from_millis(GIT_COMMAND_TIMEOUT_MS),
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(arguments)
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| GitError::Failed(format!("`git {}` timed out", arguments.join(" "))))?
    .map_err(|error| GitError::Failed(format!("git could not run: {error}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail: String = stderr
            .trim()
            .chars()
            .take(HARNESS_FAILURE_DETAIL_MAX_BYTES)
            .collect();
        return Err(GitError::Failed(detail));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::is_valid_branch;

    #[test]
    fn branches_are_boardai_slugs() {
        for good in ["boardai/fix-ttl", "boardai/a", "boardai/retry-2"] {
            assert!(is_valid_branch(good), "{good}");
        }
        for bad in [
            "main",
            "boardai/",
            "boardai/Fix",
            "boardai/a b",
            "boardai/-x",
            "boardai/x--y",
            "boardai/../main",
            "feature/boardai/x",
        ] {
            assert!(!is_valid_branch(bad), "{bad}");
        }
    }
}
