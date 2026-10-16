//! Text the core writes for the talker and says out loud: tool results, context notes, the
//! lines spoken on failures, and branch names.

use std::fmt::Write as _;

use crate::harness::{HarnessEvent, HarnessFailure, HarnessKind};
use crate::ids::TaskId;
use crate::input::{TalkerFailure, TalkerFailureKind};
use crate::limits::BRANCH_SLUG_MAX_BYTES;
use crate::tools::{InvalidToolInput, ToolFailure, ToolOutput};

/// Spoken when the model refuses (LLD §3.3): a short line, never silence.
pub const REFUSAL_LINE: &str = "I can't help with that one.";

/// Shown under the text of a clause the voice couldn't say (it is never spoken: the voice is
/// what failed).
pub const SYNTHESIS_FAILED_DETAIL: &str = "The voice couldn't say this part.";

/// "Forget that" (PRODUCT §7.4): confirmations are three words or fewer.
pub const FORGOTTEN_LINE: &str = "Forgotten.";

/// "Yes, keep it": a proposed memory is kept.
pub const KEPT_LINE: &str = "Kept.";

/// "Don't remember this session" (I10).
pub const OFF_THE_RECORD_LINE: &str = "Won't remember this.";

/// Spoken when a turn runs out of tool rounds.
pub const TOOL_ROUNDS_LINE: &str = "I got stuck looking things up. Let's try a narrower question.";

/// The spoken line for a talker failure. `talker` is the model's name as the user knows it.
#[must_use]
pub fn failure_line(failure: &TalkerFailure, talker: &str) -> String {
    match failure.kind {
        TalkerFailureKind::Network | TalkerFailureKind::Timeout => {
            format!("I lost the connection to {talker}.")
        }
        TalkerFailureKind::Auth => format!("{talker} didn't accept the API key."),
        TalkerFailureKind::RateLimited { .. } => {
            format!("{talker} is rate limiting us. Give it a moment.")
        }
        TalkerFailureKind::BadOutput => format!("I got a garbled answer from {talker}."),
        TalkerFailureKind::Provider => format!("{talker} reported an error."),
        TalkerFailureKind::NotConfigured => "No talker model is set up yet.".to_owned(),
    }
}

/// A tool result as the talker reads it, and whether it is an error.
#[must_use]
pub fn render_output(output: &ToolOutput) -> (String, bool) {
    match output {
        ToolOutput::File {
            path,
            first_line,
            lines,
            more_lines,
        } => {
            let mut text = format!("{path}\n");
            for (number, line) in (*first_line..).zip(lines) {
                let _ = writeln!(text, "{number:>5} | {line}");
            }
            if *more_lines > 0 {
                let _ = writeln!(text, "({more_lines} more lines)");
            }
            (text, false)
        }
        ToolOutput::Matches { hits, more_hits } => {
            let mut text = String::new();
            for hit in hits {
                let _ = writeln!(text, "{}:{}: {}", hit.path, hit.line, hit.text);
            }
            if hits.is_empty() {
                text.push_str("No matches.\n");
            }
            if *more_hits > 0 {
                let _ = writeln!(text, "({more_hits} more matches)");
            }
            (text, false)
        }
        ToolOutput::Listing {
            entries,
            more_entries,
        } => {
            let mut text = String::new();
            for entry in entries {
                let slash = if entry.is_dir { "/" } else { "" };
                let _ = writeln!(text, "{}{slash}", entry.name);
            }
            if *more_entries > 0 {
                let _ = writeln!(text, "({more_entries} more entries)");
            }
            (text, false)
        }
        ToolOutput::Started { task } => (started_text(*task), false),
        ToolOutput::Text { text } => (text.clone(), false),
        ToolOutput::Failed { kind, message } => {
            (format!("Error ({}): {message}", failure_name(*kind)), true)
        }
        ToolOutput::View { view, today } => (super::views::describe(view, today), false),
    }
}

#[must_use]
pub fn started_text(task: TaskId) -> String {
    format!(
        "Started task {}. It runs in the background; its result arrives later as a system \
         message. Tell the user it is running and keep going.",
        task.0
    )
}

fn failure_name(kind: ToolFailure) -> &'static str {
    match kind {
        ToolFailure::OutsideRepo => "outside the opened repo",
        ToolFailure::NotFound => "not found",
        ToolFailure::Unavailable => "unavailable",
        ToolFailure::Io => "read error",
        ToolFailure::InvalidInput => "invalid input",
    }
}

/// The error result for a call rejected at the boundary.
#[must_use]
pub fn invalid_call(error: &InvalidToolInput) -> String {
    match error {
        InvalidToolInput::UnknownTool { name } => format!("Error: there is no tool named {name}."),
        InvalidToolInput::TooLarge { size_bytes } => {
            format!("Error: the input is too large ({size_bytes} bytes).")
        }
        InvalidToolInput::Malformed { reason } => format!("Error: invalid input: {reason}."),
    }
}

#[must_use]
pub fn harness_kind_name(kind: &HarnessKind) -> &'static str {
    match kind {
        HarnessKind::Research => "Research",
        HarnessKind::Review => "Review",
        HarnessKind::Change { .. } => "Change",
    }
}

/// The system message for a harness result; `None` for progress, which only shows on screen.
#[must_use]
pub fn harness_report(task: TaskId, kind: &HarnessKind, event: &HarnessEvent) -> Option<String> {
    let name = harness_kind_name(kind);
    match event {
        HarnessEvent::Progress { .. } | HarnessEvent::FileTouched { .. } => None,
        HarnessEvent::Finished { summary, citations } => {
            let mut text = format!("{name} task {} finished: {summary}", task.0);
            if !citations.is_empty() {
                let _ = write!(text, "\nSources: {}", citations.join(", "));
            }
            Some(text)
        }
        HarnessEvent::Failed { reason, detail } => Some(format!(
            "{name} task {} failed ({}): {detail}",
            task.0,
            harness_failure_name(*reason)
        )),
    }
}

fn harness_failure_name(reason: HarnessFailure) -> &'static str {
    match reason {
        HarnessFailure::NotInstalled => "no coding harness is installed",
        HarnessFailure::TimedOut => "timed out",
        HarnessFailure::Cancelled => "cancelled",
        HarnessFailure::Crashed => "crashed",
        HarnessFailure::BadOutput => "unreadable output",
        HarnessFailure::QueueFull => "too many tasks are running; tell the user it was refused",
    }
}

/// `boardai/<slug>` for a change: lowercase words joined by dashes, cut at a word.
#[must_use]
pub fn branch_for(instruction: &str) -> String {
    let mut slug = String::new();
    for word in instruction
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
    {
        let extra = usize::from(!slug.is_empty()) + word.len();
        if slug.len() + extra > BRANCH_SLUG_MAX_BYTES {
            break;
        }
        if !slug.is_empty() {
            slug.push('-');
        }
        slug.push_str(&word.to_ascii_lowercase());
    }
    if slug.is_empty() {
        slug.push_str("change");
    }
    format!("boardai/{slug}")
}

#[cfg(test)]
mod tests {
    use super::{branch_for, failure_line, render_output};
    use crate::input::{TalkerFailure, TalkerFailureKind};
    use crate::limits::BRANCH_SLUG_MAX_BYTES;
    use crate::tools::{ToolFailure, ToolOutput};

    #[test]
    fn branch_slugs_are_bounded_and_never_empty() {
        assert_eq!(
            branch_for("Add jitter to the retry policy!"),
            "boardai/add-jitter-to-the-retry-policy"
        );
        assert_eq!(branch_for("¿¿"), "boardai/change");
        let long = branch_for(&"word ".repeat(40));
        assert!(long.len() <= "boardai/".len() + BRANCH_SLUG_MAX_BYTES);
    }

    #[test]
    fn files_render_with_line_numbers_and_failures_as_errors() {
        let file = ToolOutput::File {
            path: "lb.go".into(),
            first_line: 41,
            lines: vec!["func pick() {".into()],
            more_lines: 3,
        };
        let (text, is_error) = render_output(&file);
        assert!(text.contains("   41 | func pick() {") && text.contains("(3 more lines)"));
        assert!(!is_error);
        let failed = ToolOutput::Failed {
            kind: ToolFailure::NotFound,
            message: "no such file".into(),
        };
        assert!(render_output(&failed).1);
    }

    /// PRODUCT §7.4: BoardAI confirms memory controls in three words or fewer.
    #[test]
    fn memory_confirmations_are_three_words_or_fewer() {
        for line in [
            super::FORGOTTEN_LINE,
            super::KEPT_LINE,
            super::OFF_THE_RECORD_LINE,
        ] {
            assert!(line.split_whitespace().count() <= 3, "{line}");
        }
    }

    #[test]
    fn every_failure_kind_has_a_spoken_line() {
        for kind in [
            TalkerFailureKind::Auth,
            TalkerFailureKind::RateLimited {
                retry_after_ms: None,
            },
            TalkerFailureKind::Network,
            TalkerFailureKind::Timeout,
            TalkerFailureKind::BadOutput,
            TalkerFailureKind::Provider,
            TalkerFailureKind::NotConfigured,
        ] {
            let failure = TalkerFailure {
                kind,
                detail: String::new(),
            };
            assert!(!failure_line(&failure, "Claude").is_empty());
        }
        let network = TalkerFailure {
            kind: TalkerFailureKind::Network,
            detail: String::new(),
        };
        assert_eq!(
            failure_line(&network, "Claude"),
            "I lost the connection to Claude."
        );
    }
}
