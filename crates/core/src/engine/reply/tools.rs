//! The talker's tool calls and the coding harness's reports within a turn (LLD §3.3, §3.5).

use crate::effect::Effect;
use crate::engine::Core;
use crate::engine::gate::GateClosed;
use crate::engine::nudge::is_view;
use crate::engine::reply::ToolSlot;
use crate::engine::text::{
    TOOL_ROUNDS_LINE, harness_report, invalid_call, render_output, started_text,
};
use crate::harness::{HarnessEvent, HarnessKind, HarnessTask};
use crate::history::UserPart;
use crate::ids::{TaskId, ToolCallId};
use crate::input::StopReason;
use crate::limits::{TOOL_CALLS_PER_RESPONSE_MAX, TOOL_ROUNDS_PER_TURN_MAX};
use crate::protocol::{ChipKind, UiEvent};
use crate::time::SessionTime;
use crate::tools::{ToolCall, ToolInvocation, ToolOutput};

impl Core {
    pub(in crate::engine) fn on_tool_call(
        &mut self,
        now: SessionTime,
        invocation: ToolInvocation,
        effects: &mut Vec<Effect>,
    ) {
        let Some(reply) = self.reply.as_mut().filter(|reply| reply.in_flight) else {
            return;
        };
        let too_many = reply.tools.len() >= TOOL_CALLS_PER_RESPONSE_MAX;
        if let Some(stream) = reply.streams.last_mut() {
            stream.calls.push(invocation.clone());
        }
        let result = match (&invocation.call, too_many) {
            (_, true) => Some(error_result(
                &invocation.id,
                "Error: too many tool calls in one reply.".to_owned(),
            )),
            (Err(error), false) => Some(error_result(&invocation.id, invalid_call(error))),
            (Ok(call), false) => self.dispatch_tool(now, &invocation.id, call, effects),
        };
        if let Some(reply) = self.reply.as_mut() {
            reply.tools.push(ToolSlot {
                id: invocation.id,
                result,
            });
        }
    }

    /// Runs a tool call; returns the result now when the core answers it itself.
    fn dispatch_tool(
        &mut self,
        now: SessionTime,
        id: &ToolCallId,
        call: &ToolCall,
        effects: &mut Vec<Effect>,
    ) -> Option<UserPart> {
        let text = match call {
            ToolCall::ReadFile { .. }
            | ToolCall::SearchCode { .. }
            | ToolCall::ListDir { .. }
            | ToolCall::Recall { .. }
            | ToolCall::OpenSession { .. }
            | ToolCall::ShowLibrary
            | ToolCall::ShowTasks { .. }
            | ToolCall::ShowMemory
            | ToolCall::PlaySession { .. } => {
                if let Some(reply) = self.reply.as_mut() {
                    reply.shown |= is_view(call);
                    reply.touring |= matches!(
                        call,
                        ToolCall::ReadFile { .. }
                            | ToolCall::SearchCode { .. }
                            | ToolCall::ListDir { .. }
                    );
                }
                effects.push(Effect::RunTool {
                    id: id.clone(),
                    call: call.clone(),
                });
                return None;
            }
            ToolCall::Research { question, depth } => {
                let brief = format!("{question}\n(depth: {depth:?})");
                self.run_read_only(HarnessKind::Research, brief, effects)
            }
            ToolCall::Review { scope } => {
                self.run_read_only(HarnessKind::Review, scope.clone(), effects)
            }
            ToolCall::ProposeChange { instruction } => {
                let stream = self
                    .reply
                    .as_ref()
                    .map_or(0, |reply| reply.streams.len().saturating_sub(1));
                return Some(self.propose_change(id, instruction, stream));
            }
            ToolCall::Remember {
                kind,
                text,
                about,
                links,
            } => self.remember(now, *kind, text, *about, links, effects),
            ToolCall::AddTask {
                title,
                project,
                links,
            } => self.add_task(title, project.clone(), links, effects),
        };
        Some(UserPart::ToolResult {
            id: id.clone(),
            content: text,
            is_error: false,
        })
    }

    fn run_read_only(
        &mut self,
        kind: HarnessKind,
        brief: String,
        effects: &mut Vec<Effect>,
    ) -> String {
        let task = self.start_task(kind.clone());
        effects.push(Effect::RunHarness {
            task,
            spec: HarnessTask { kind, brief },
        });
        started_text(task)
    }

    pub(in crate::engine) fn start_task(&mut self, kind: HarnessKind) -> TaskId {
        let task = TaskId(self.next_task);
        self.next_task = self.next_task.wrapping_add(1);
        self.context.tasks.insert(task, kind);
        task
    }

    pub(in crate::engine) fn on_tool_result(
        &mut self,
        now: SessionTime,
        id: &ToolCallId,
        output: ToolOutput,
        effects: &mut Vec<Effect>,
    ) {
        let Some(output) = self.on_written_read(id, output, effects) else {
            return;
        };
        let listed = self.listed_folder(id);
        self.repo_names.record(listed.as_deref(), &output);
        let Some(slot) = self.reply.as_mut().and_then(|reply| {
            reply
                .tools
                .iter_mut()
                .find(|slot| slot.id == *id && slot.result.is_none())
        }) else {
            return;
        };
        let (content, is_error) = render_output(&output);
        slot.result = Some(UserPart::ToolResult {
            id: id.clone(),
            content,
            is_error,
        });
        if let ToolOutput::File {
            path,
            first_line,
            lines,
            ..
        } = &output
        {
            self.surfaces.record_read(path, *first_line, lines);
        }
        if let ToolOutput::View { view, today } = output {
            self.show_view(*view, today, effects);
        }
        self.maybe_tool_round(now, effects);
    }

    /// The folder call `id` of the reply listed, when it was a `list_dir`.
    fn listed_folder(&self, id: &ToolCallId) -> Option<String> {
        let reply = self.reply.as_ref()?;
        reply
            .streams
            .iter()
            .flat_map(|stream| &stream.calls)
            .find(|call| call.id == *id)
            .and_then(|call| {
                if let Ok(ToolCall::ListDir { path }) = &call.call {
                    Some(path.clone())
                } else {
                    None
                }
            })
    }

    /// When the stream stopped for tools and every result is in: store the assistant message
    /// and the results, then ask again.
    pub(super) fn maybe_tool_round(&mut self, now: SessionTime, effects: &mut Vec<Effect>) {
        let Some(reply) = self.reply.as_ref() else {
            return;
        };
        let stopped_for_tools = reply
            .streams
            .last()
            .is_some_and(|stream| stream.stop == Some(StopReason::ToolUse));
        let all_in = reply.tools.iter().all(|slot| slot.result.is_some());
        if reply.in_flight || !stopped_for_tools || !all_in {
            return;
        }
        let index = reply.streams.len() - 1;
        self.commit_stream(index);
        let Some(reply) = self.reply.as_mut() else {
            return;
        };
        reply.rounds += 1;
        if reply.rounds >= TOOL_ROUNDS_PER_TURN_MAX {
            if let Some(reply) = self.reply.as_mut() {
                reply.done = true;
            }
            self.speak_problem(
                TOOL_ROUNDS_LINE,
                "The turn reached its tool round limit.",
                effects,
            );
            return;
        }
        self.request_talk(now, effects);
    }

    pub(in crate::engine) fn on_harness(
        &mut self,
        task: TaskId,
        event: HarnessEvent,
        effects: &mut Vec<Effect>,
    ) {
        // A task that was cancelled (or already reported) has nothing more to say.
        let Some(kind) = self.context.tasks.get(&task).cloned() else {
            return;
        };
        if let HarnessEvent::FileTouched { path } = &event {
            // A harness names a file, never a line it read here: the chip opens nothing.
            effects.push(Effect::Ui(UiEvent::Chip {
                kind: ChipKind::Source,
                text: path.clone(),
                source: None,
            }));
        }
        if let Some(report) = harness_report(task, &kind, &event) {
            self.context.tasks.remove(&task);
            self.context.report(report);
        }
    }

    /// Harness results are reported when the conversation is idle (LLD §3.3).
    pub(in crate::engine) fn maybe_report(&mut self, now: SessionTime, effects: &mut Vec<Effect>) {
        // While the user draws, BoardAI waits: a report goes out once they finish (I14).
        let idle = self.state == crate::conversation::ConversationState::Idle
            && self.reply.is_none()
            && self.stop.is_none()
            && !self.has_ended()
            && !self.is_drawing();
        if idle && !self.context.reports.is_empty() {
            // The report turn asks its own questions: a yes to one of them must not answer
            // BoardAI's question about a change (I9).
            self.close_change_gate(GateClosed::Report);
            self.marks = crate::engine::TurnMarks::default();
            self.turn_question = None;
            self.state = crate::conversation::ConversationState::Thinking;
            self.request_talk(now, effects);
        }
    }
}

pub(super) fn error_result(id: &ToolCallId, content: String) -> UserPart {
    UserPart::ToolResult {
        id: id.clone(),
        content,
        is_error: true,
    }
}
