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
//!   and shown, as a call's would be. The read belongs to its reply: a stop or the pen drops it,
//!   as a call's result is dropped, so a view never opens over the user's interruption or their
//!   drawing;
//! - any other tool runs nothing: its result is for the talker, and no call of its awaits one in
//!   the history (I12), or it needs a yes (I9).
//!
//! The talker hears what happened in a notice with its next request, and to call tools as
//! tools: for a view, once its read has answered (shown, or why not). Tags past
//! `TOOL_CALLS_PER_RESPONSE_MAX` are not read, and the notice counts them. Nothing is added to
//! the history but those notices: the tag stays text, as written.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::effect::Effect;
use crate::engine::Core;
use crate::engine::text::render_output;
use crate::ids::ToolCallId;
use crate::limits::{LABEL_MAX_BYTES, TOOL_CALLS_PER_RESPONSE_MAX};
use crate::narration::parser::{Piece, TagParser};
use crate::narration::tags::bounded;
use crate::tools::{InvalidToolInput, TOOL_NAMES, ToolCall, ToolOutput};

/// The prefix of the core's own ids for the reads it makes for tags (`written-3`).
const WRITTEN_ID_PREFIX: &str = "written-";

/// A tag named after a talker tool, as written, and what its attributes parse to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct WrittenCall {
    pub written: String,
    pub name: String,
    pub call: Result<ToolCall, InvalidToolInput>,
}

/// Views read for tags, waiting for the runtime's answer, by number, with each tag as written
/// (bounded). Their ids are the core's own (`written-3`), never a provider's, so they can't
/// answer a call in the history; the number orders them, so the oldest goes first past the
/// bound ("written-10" sorts before "written-9" as text).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct WrittenReads {
    pending: BTreeMap<u32, String>,
    next: u32,
}

/// The tool tags a stream wrote: the first `TOOL_CALLS_PER_RESPONSE_MAX`, in order, and how many
/// more there were, which are not read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct WrittenCalls {
    pub calls: Vec<WrittenCall>,
    pub over: usize,
}

/// The tags in `text` named after a talker tool. The narration parser decides what is a tag, so
/// text that only resembles one ("a < b") is never read as a call.
#[must_use]
pub(super) fn written_calls(text: &str) -> WrittenCalls {
    let mut parser = TagParser::new();
    let mut pieces = Vec::new();
    parser.push(text, &mut pieces);
    parser.finish(&mut pieces);
    let mut written = WrittenCalls::default();
    for piece in pieces {
        let call = match piece {
            Piece::Dropped(text) => tool_tag(text),
            Piece::Text(_) | Piece::Tag(_) => None,
        };
        let Some(call) = call else {
            continue;
        };
        if written.calls.len() < TOOL_CALLS_PER_RESPONSE_MAX {
            written.calls.push(call);
        } else {
            written.over += 1;
        }
    }
    written
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
        let WrittenCalls { calls, over } = written_calls(&written);
        let mut lines: Vec<String> = calls
            .into_iter()
            .filter_map(|call| self.carry_out(call, effects))
            .collect();
        if over > 0 {
            lines.push(format!(
                "- {over} more tool tags were over the limit of {TOOL_CALLS_PER_RESPONSE_MAX} per \
                 reply and were not read."
            ));
        }
        if lines.is_empty() {
            return;
        }
        self.context.notice(format!(
            "Your last reply wrote tool calls as tags. Tags are only for drawing, so a tool \
             written as a tag is not a call; call tools as tools.\n{}",
            lines.join("\n")
        ));
    }

    /// Carries one tag out; returns the talker's line about it now, or `None` for a view, whose
    /// line waits for its read (`on_written_read`).
    fn carry_out(&mut self, written: WrittenCall, effects: &mut Vec<Effect>) -> Option<String> {
        let tag = bounded(&written.written, LABEL_MAX_BYTES);
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
                self.read_written_view(call, tag, effects);
                return None;
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
        Some(format!("- {tag}: {outcome}"))
    }

    /// Tells the talker about tool calls stream `index` wrote as words (D-179): they were kept
    /// out of speech, and nothing ran.
    pub(super) fn notice_spoken_calls(&mut self, index: usize) {
        let Some(calls) = self
            .reply
            .as_ref()
            .and_then(|reply| reply.streams.get(index))
            .map(|stream| stream.narration.spoken_calls())
            .filter(|calls| !calls.is_empty())
        else {
            return;
        };
        let lines: Vec<String> = calls
            .iter()
            .map(|call| format!("- {}", bounded(&call.written, LABEL_MAX_BYTES)))
            .collect();
        self.context.notice(format!(
            "Your last reply wrote tool calls as words. They were not spoken and nothing ran: \
             call tools as tools, never in your speech.\n{}",
            lines.join("\n")
        ));
    }

    /// Reads a view for a tag; the answer shows it (`on_written_read`).
    fn read_written_view(&mut self, call: ToolCall, tag: String, effects: &mut Vec<Effect>) {
        if let Some(reply) = self.reply.as_mut() {
            reply.shown = true;
        }
        let reads = &mut self.written_reads;
        let number = reads.next;
        let Some(id) = ToolCallId::parse(&format!("{WRITTEN_ID_PREFIX}{number}")) else {
            return;
        };
        reads.next = reads.next.wrapping_add(1);
        // A read the runtime never answers is forgotten past the bound, oldest first.
        while reads.pending.len() >= TOOL_CALLS_PER_RESPONSE_MAX {
            reads.pending.pop_first();
        }
        reads.pending.insert(number, tag);
        effects.push(Effect::RunTool { id, call });
    }

    /// The reply the reads were made for stopped (an interruption, Esc, mute, the pen): their
    /// answers show nothing, as the results of its calls are dropped.
    pub(super) fn drop_written_reads(&mut self) {
        self.written_reads.pending.clear();
    }

    /// The runtime answered a read made for a tag: a view is shown, and the talker hears what
    /// came of the tag, shown or why not. The output comes back when `id` is not such a read; an
    /// answer for a read its reply dropped is ignored.
    pub(super) fn on_written_read(
        &mut self,
        id: &ToolCallId,
        output: ToolOutput,
        effects: &mut Vec<Effect>,
    ) -> Option<ToolOutput> {
        let Some(number) = id
            .as_str()
            .strip_prefix(WRITTEN_ID_PREFIX)
            .and_then(|number| number.parse::<u32>().ok())
        else {
            return Some(output);
        };
        let tag = self.written_reads.pending.remove(&number)?;
        let outcome = if let ToolOutput::View { view, today } = output {
            let kind = crate::engine::views::kind_name(view.kind());
            self.show_view(*view, today, effects);
            format!("the {kind} view is on screen")
        } else {
            let (said, _) = render_output(&output);
            format!("nothing was shown: {}", bounded(&said, LABEL_MAX_BYTES))
        };
        self.context.notice(format!(
            "Your last reply wrote {tag} as a tag, so it was read as a call: {outcome}. Tags are \
             only for drawing; call tools as tools."
        ));
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{read_tag, written_calls};
    use crate::engine::{Core, CoreConfig};
    use crate::limits::TOOL_CALLS_PER_RESPONSE_MAX;
    use crate::memory::Subject;
    use crate::tools::{RememberKind, ToolCall};

    /// The owner's session 3 (2026-10-16): gemma4 wrote the call as a tag.
    #[test]
    fn a_tool_written_as_a_tag_is_read_as_its_call() {
        let text = "I can create that note for you. <remember kind=\"note\" text=\"story \
                    consensus protocol\" about=\"you\"/> I have made a note for you.";
        let calls = written_calls(text).calls;
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
        let shown = written_calls("Here is what I remember. <show_memory/>").calls;
        assert_eq!(shown[0].call, Ok(ToolCall::ShowMemory));
        let task = written_calls("<add_task title='Read Raft &amp; Paxos' links=\"a | b\"/>").calls;
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
            assert!(written_calls(text).calls.is_empty(), "{text}");
        }
    }

    #[test]
    fn a_broken_tool_tag_is_reported_not_run() {
        let calls = written_calls("<remember kind=\"note\" text=unquoted/>").calls;
        assert_eq!(calls.len(), 1);
        assert!(calls[0].call.is_err());
        let unknown = written_calls("<remember kind=\"diary\" text=\"x\"/>").calls;
        assert!(
            unknown[0].call.is_err(),
            "an unknown kind fails the tool's own parse"
        );
        assert_eq!(read_tag("<>"), None);
    }

    #[test]
    fn at_most_the_per_reply_bound_is_read_and_the_rest_counted() {
        let text = "<show_memory/>".repeat(TOOL_CALLS_PER_RESPONSE_MAX + 4);
        let written = written_calls(&text);
        assert_eq!(written.calls.len(), TOOL_CALLS_PER_RESPONSE_MAX);
        assert_eq!(written.over, 4);
    }

    /// Past the bound the oldest read is forgotten: by number, so `written-10` outlives
    /// `written-9` though it sorts first as text.
    #[test]
    fn the_oldest_read_is_forgotten_first() {
        let mut core = Core::new(CoreConfig::default());
        let mut effects = Vec::new();
        for _ in 0..TOOL_CALLS_PER_RESPONSE_MAX + 1 {
            core.read_written_view(
                ToolCall::ShowMemory,
                "<show_memory/>".to_owned(),
                &mut effects,
            );
        }
        let numbers: Vec<u32> = core.written_reads.pending.keys().copied().collect();
        let expected: Vec<u32> =
            (1..=u32::try_from(TOOL_CALLS_PER_RESPONSE_MAX).expect("small")).collect();
        assert_eq!(numbers, expected, "written-0 goes first");
    }
}
