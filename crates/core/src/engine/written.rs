//! Tool calls the talker wrote as tags (D-143).
//!
//! A local model sometimes writes a tool call into its speech as a tag (`<remember kind="note"
//! text="…"/>`, `<show_memory/>`) instead of calling the tool. The narration parser drops an
//! unknown tag (LLD §2.3), so the call was lost while BoardAI said "I have made a note": in the
//! owner's third session gemma4 wrote every tool call this way, and no note was kept.
//!
//! When a talker stream ends, the core reads what it wrote for tags named after a talker tool:
//! - `remember` and `add_task` are carried out as if called (the same policy, chip and capture);
//!   an inferred memory waits in the digest, since nobody asked the user about it;
//! - a view (`show_library`, `show_tasks`, `show_memory`, `open_session`, `play_session`) is read
//!   and shown, as a call's would be;
//! - any other tool runs nothing: its result is for the talker, and no call of its awaits one in
//!   the history (I12), or it needs a yes (I9).
//!
//! The talker hears what happened in a notice with its next request, and to call tools as
//! tools. Nothing is added to the history but that notice: the tag stays text, as written.

use std::collections::BTreeSet;

use serde_json::{Map, Value};

use crate::effect::Effect;
use crate::engine::Core;
use crate::ids::ToolCallId;
use crate::limits::{LABEL_MAX_BYTES, TOOL_CALLS_PER_RESPONSE_MAX};
use crate::narration::parser::{Piece, TagParser};
use crate::narration::tags::bounded;
use crate::tools::{InvalidToolInput, TOOL_NAMES, ToolCall, ToolOutput};

/// A tag named after a talker tool, as written, and what its attributes parse to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct WrittenCall {
    pub written: String,
    pub name: String,
    pub call: Result<ToolCall, InvalidToolInput>,
}

/// Views read for tags, waiting for the runtime's answer. Their ids are the core's own
/// (`written-3`), never a provider's, so they can't answer a call in the history.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct WrittenReads {
    pending: BTreeSet<ToolCallId>,
    next: u32,
}

/// The tags in `text` named after a talker tool, in order, at most
/// `TOOL_CALLS_PER_RESPONSE_MAX`. The narration parser decides what is a tag, so text that only
/// resembles one ("a < b") is never read as a call.
#[must_use]
pub(super) fn written_calls(text: &str) -> Vec<WrittenCall> {
    let mut parser = TagParser::new();
    let mut pieces = Vec::new();
    parser.push(text, &mut pieces);
    parser.finish(&mut pieces);
    pieces
        .into_iter()
        .filter_map(|piece| match piece {
            Piece::Dropped(written) => tool_tag(written),
            Piece::Text(_) | Piece::Tag(_) => None,
        })
        .take(TOOL_CALLS_PER_RESPONSE_MAX)
        .collect()
}

fn tool_tag(written: String) -> Option<WrittenCall> {
    let (name, attributes) = read_tag(&written)?;
    let name = TOOL_NAMES.iter().find(|tool| **tool == name)?;
    let call = match attributes {
        Some(attributes) => ToolCall::parse(name, &input_json(&attributes)),
        None => Err(InvalidToolInput::Malformed {
            reason: "the tag's attributes could not be read".to_owned(),
        }),
    };
    Some(WrittenCall {
        written,
        name: (*name).to_owned(),
        call,
    })
}

/// A tag's attributes, in order: `(name, value)`.
type Attributes = Vec<(String, String)>;

fn is_name_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'
}

/// `<name a="1" b='2'/>` as its lowercase name and attributes; the attributes are `None` when
/// they can't be read. `None` when there is no name. Each pass of the loop consumes a whole
/// attribute or ends, so it is bounded by the tag's length (`NARRATION_TAG_MAX_BYTES`).
fn read_tag(written: &str) -> Option<(String, Option<Attributes>)> {
    let rest = written.strip_prefix('<')?;
    let name_end = rest
        .find(|ch: char| !is_name_char(ch))
        .unwrap_or(rest.len());
    let (name, mut rest) = rest.split_at(name_end);
    if name.is_empty() {
        return None;
    }
    let name = name.to_ascii_lowercase();
    let mut attributes = Vec::new();
    loop {
        rest = rest.trim_start();
        if rest.is_empty() || rest.starts_with('/') || rest.starts_with('>') {
            return Some((name, Some(attributes)));
        }
        let Some((key, value, after)) = read_attribute(rest) else {
            return Some((name, None));
        };
        attributes.push((key, value));
        rest = after;
    }
}

/// `key="value"` at the start of `text`, then the rest.
fn read_attribute(text: &str) -> Option<(String, String, &str)> {
    let key_end = text
        .find(|ch: char| !is_name_char(ch))
        .unwrap_or(text.len());
    if key_end == 0 {
        return None;
    }
    let (key, after) = text.split_at(key_end);
    let after = after.trim_start().strip_prefix('=')?.trim_start();
    let quote = after
        .chars()
        .next()
        .filter(|ch| *ch == '"' || *ch == '\'')?;
    let value = &after[quote.len_utf8()..];
    let close = value.find(quote)?;
    Some((
        key.to_ascii_lowercase(),
        decode(&value[..close]),
        &value[close + quote.len_utf8()..],
    ))
}

/// The XML entities the narration parser decodes in values.
fn decode(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// The attributes as a tool's JSON input: strings, and `links` split on `|` into a list.
fn input_json(attributes: &Attributes) -> String {
    let mut fields = Map::new();
    for (key, value) in attributes {
        let value = if key == "links" {
            Value::Array(
                value
                    .split('|')
                    .map(str::trim)
                    .filter(|link| !link.is_empty())
                    .map(|link| Value::String(link.to_owned()))
                    .collect(),
            )
        } else {
            Value::String(value.clone())
        };
        fields.insert(key.clone(), value);
    }
    Value::Object(fields).to_string()
}

impl Core {
    /// Carries out the tool calls stream `index` wrote as tags, and tells the talker.
    pub(super) fn carry_out_written_calls(&mut self, index: usize, effects: &mut Vec<Effect>) {
        let Some(written) = self
            .reply
            .as_ref()
            .and_then(|reply| reply.streams.get(index))
            .map(|stream| stream.narration.written().to_owned())
        else {
            return;
        };
        let calls = written_calls(&written);
        if calls.is_empty() {
            return;
        }
        let lines: Vec<String> = calls
            .into_iter()
            .map(|call| self.carry_out(call, effects))
            .collect();
        self.context.notice(format!(
            "Your last reply wrote tool calls as tags. Tags are only for drawing, so a tool \
             written as a tag is not a call; call tools as tools.\n{}",
            lines.join("\n")
        ));
    }

    fn carry_out(&mut self, written: WrittenCall, effects: &mut Vec<Effect>) -> String {
        let outcome = match written.call {
            Ok(ToolCall::Remember {
                kind,
                text,
                about,
                links,
            }) => format!(
                "carried out as a call: {}",
                self.remember_written(kind, &text, about, &links, effects)
            ),
            Ok(ToolCall::AddTask {
                title,
                project,
                links,
            }) => format!(
                "carried out as a call: {}",
                self.add_task(&title, project, &links, effects)
            ),
            Ok(
                call @ (ToolCall::ShowLibrary
                | ToolCall::ShowTasks { .. }
                | ToolCall::ShowMemory
                | ToolCall::OpenSession { .. }
                | ToolCall::PlaySession { .. }),
            ) => {
                self.read_written_view(call, effects);
                "carried out as a call: the view is on screen.".to_owned()
            }
            Ok(
                ToolCall::ReadFile { .. }
                | ToolCall::SearchCode { .. }
                | ToolCall::ListDir { .. }
                | ToolCall::Recall { .. }
                | ToolCall::Research { .. }
                | ToolCall::Review { .. }
                | ToolCall::ProposeChange { .. },
            ) => format!(
                "nothing ran. Call {} as a tool if it is still needed.",
                written.name
            ),
            Err(_) => "nothing ran: its attributes don't fit the tool.".to_owned(),
        };
        format!(
            "- {}: {outcome}",
            bounded(&written.written, LABEL_MAX_BYTES)
        )
    }

    /// Reads a view for a tag; the answer shows it (`on_written_read`).
    fn read_written_view(&mut self, call: ToolCall, effects: &mut Vec<Effect>) {
        let reads = &mut self.written_reads;
        let Some(id) = ToolCallId::parse(&format!("written-{}", reads.next)) else {
            return;
        };
        reads.next = reads.next.wrapping_add(1);
        // A read the runtime never answers is forgotten past the bound, oldest first.
        while reads.pending.len() >= TOOL_CALLS_PER_RESPONSE_MAX {
            reads.pending.pop_first();
        }
        reads.pending.insert(id.clone());
        effects.push(Effect::RunTool { id, call });
    }

    /// The runtime answered a read made for a tag: a view is shown, anything else (a failed
    /// read) is dropped, since no call of the talker's waits for it. The output comes back when
    /// `id` is not such a read.
    pub(super) fn on_written_read(
        &mut self,
        id: &ToolCallId,
        output: ToolOutput,
        effects: &mut Vec<Effect>,
    ) -> Option<ToolOutput> {
        if !self.written_reads.pending.remove(id) {
            return Some(output);
        }
        if let ToolOutput::View { view, today } = output {
            self.show_view(*view, today, effects);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{read_tag, written_calls};
    use crate::limits::TOOL_CALLS_PER_RESPONSE_MAX;
    use crate::memory::Subject;
    use crate::tools::{RememberKind, ToolCall};

    /// The owner's session 3 (2026-10-16): gemma4 wrote the call as a tag.
    #[test]
    fn a_tool_written_as_a_tag_is_read_as_its_call() {
        let text = "I can create that note for you. <remember kind=\"note\" text=\"story \
                    consensus protocol\" about=\"you\"/> I have made a note for you.";
        let calls = written_calls(text);
        assert_eq!(calls.len(), 1, "{calls:?}");
        assert_eq!(
            calls[0].call,
            Ok(ToolCall::Remember {
                kind: RememberKind::Note,
                text: "story consensus protocol".to_owned(),
                about: Subject::You,
                links: Vec::new(),
            })
        );
        let shown = written_calls("Here is what I remember. <show_memory/>");
        assert_eq!(shown[0].call, Ok(ToolCall::ShowMemory));
        let task = written_calls("<add_task title='Read Raft &amp; Paxos' links=\"a | b\"/>");
        assert_eq!(
            task[0].call,
            Ok(ToolCall::AddTask {
                title: "Read Raft & Paxos".to_owned(),
                project: None,
                links: vec!["a".to_owned(), "b".to_owned()],
            })
        );
    }

    #[test]
    fn drawing_tags_and_lookalikes_are_not_calls() {
        for text in [
            "Then the <node id=\"lb\" label=\"LB\"/>load balancer.",
            "if a < b then remember it",
            "count<limit and <unknown x=\"1\"/> stays dropped",
            "plain speech",
        ] {
            assert!(written_calls(text).is_empty(), "{text}");
        }
    }

    #[test]
    fn a_broken_tool_tag_is_reported_not_run() {
        let calls = written_calls("<remember kind=\"note\" text=unquoted/>");
        assert_eq!(calls.len(), 1);
        assert!(calls[0].call.is_err());
        let unknown = written_calls("<remember kind=\"diary\" text=\"x\"/>");
        assert!(
            unknown[0].call.is_err(),
            "an unknown kind fails the tool's own parse"
        );
        assert_eq!(read_tag("<>"), None);
    }

    #[test]
    fn at_most_the_per_reply_bound_is_read() {
        let text = "<show_memory/>".repeat(TOOL_CALLS_PER_RESPONSE_MAX + 4);
        assert_eq!(written_calls(&text).len(), TOOL_CALLS_PER_RESPONSE_MAX);
    }
}
