//! Law 6 for source chips: a chip that opens code names a line the session read, through a
//! repo tool or a brief's checked citation (D-123, D-185). An unread line, such as a citation
//! the native read could not find, never gets one.

use std::collections::{BTreeMap, BTreeSet};

use crate::input::Input;
use crate::protocol::{ChipKind, UiEvent};
use crate::sources::SourceInput;
use crate::time::SessionTime;
use crate::tools::ToolOutput;

#[derive(Clone, Debug, Default)]
pub struct ReadLines {
    files: BTreeMap<String, BTreeSet<u32>>,
}

impl ReadLines {
    pub fn on_input(&mut self, input: &Input) {
        if let Input::ToolResult {
            output:
                ToolOutput::File {
                    path,
                    first_line,
                    lines,
                    ..
                },
            ..
        } = input
        {
            self.record(path, *first_line, lines.len());
        }
        if let Input::Source {
            source: SourceInput::CitationsChecked { read, .. },
        } = input
        {
            for lines in read {
                self.record(&lines.path, lines.first_line, lines.lines.len());
            }
        }
    }

    fn record(&mut self, path: &str, first_line: u32, count: usize) {
        let count = u32::try_from(count).unwrap_or(u32::MAX);
        let lines = self
            .files
            .entry(path.trim_start_matches("./").to_owned())
            .or_default();
        lines.extend(first_line..first_line.saturating_add(count));
    }

    /// Whether `path:line` was read, by the path itself or a read path it ends.
    fn was_read(&self, path: &str, line: u32) -> bool {
        let path = path.trim_start_matches("./");
        let suffix = format!("/{path}");
        self.files.iter().any(|(read, lines)| {
            (read == path || read.ends_with(&suffix) || path.ends_with(&format!("/{read}")))
                && lines.contains(&line)
        })
    }

    pub fn on_ui(&self, now: SessionTime, event: &UiEvent, violations: &mut Vec<String>) {
        let UiEvent::Chip {
            kind: ChipKind::Source,
            source: Some(source),
            ..
        } = event
        else {
            return;
        };
        if !self.was_read(&source.path, source.line) {
            violations.push(format!(
                "{} ms: law 6: a source chip opens {}:{}, a line never read",
                now.as_ms(),
                source.path,
                source.line
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ReadLines;
    use crate::input::Input;
    use crate::protocol::{ChipKind, SourceRef, UiEvent};
    use crate::sources::{CitedLines, SourceInput};
    use crate::time::SessionTime;

    fn chip(path: &str, line: u32) -> UiEvent {
        UiEvent::Chip {
            kind: ChipKind::Source,
            text: format!("{path}:{line}"),
            source: Some(SourceRef {
                path: path.to_owned(),
                line,
            }),
        }
    }

    /// The planted violation: a chip for a line nothing read turns the check red.
    #[test]
    fn a_chip_for_an_unread_line_is_a_violation() {
        let mut read = ReadLines::default();
        let mut violations = Vec::new();
        read.on_ui(SessionTime::ZERO, &chip("src/arena.rs", 4), &mut violations);
        assert_eq!(violations.len(), 1, "{violations:?}");
        read.on_input(&Input::Source {
            source: SourceInput::CitationsChecked {
                task: crate::ids::TaskId(1),
                read: vec![CitedLines {
                    path: "src/arena.rs".into(),
                    first_line: 3,
                    lines: vec!["a".into(), "b".into()],
                }],
                unverified: Vec::new(),
            },
        });
        violations.clear();
        read.on_ui(SessionTime::ZERO, &chip("src/arena.rs", 4), &mut violations);
        read.on_ui(SessionTime::ZERO, &chip("arena.rs", 3), &mut violations);
        assert!(violations.is_empty(), "{violations:?}");
        read.on_ui(SessionTime::ZERO, &chip("src/arena.rs", 5), &mut violations);
        assert_eq!(violations.len(), 1);
    }
}
