//! Prints the session as it happens, stamped with session time, and keeps the numbers spikes
//! S3 and S5 need per answer: time to first token, first audio and first op, ops drawn,
//! narration tags dropped, and how well the narration follows the drawing rules (words spoken
//! before the first drawing tag, tags standing in for a word, tags piled on one word, markup).
//!
//! It is the session's store: everything the core decides passes through a store, so the
//! printer sees inputs, UI events and latency marks in order, at their step's time.

use std::sync::Mutex;

use boardai_core::effect::{Dropped, ForgetScope, LogRecord, Milestone};
use boardai_core::ids::CaptureId;
use boardai_core::input::Input;
use boardai_core::memory::MemoryWrite;
use boardai_core::narration::quality::{NarrationQuality, assess};
use boardai_core::narration::tags::TAG_NAMES;
use boardai_core::protocol::{DrawOp, OrbState, Speaker, Surface, UiEvent};
use boardai_core::time::SessionTime;
use boardai_core::tools::ToolOutput;
use boardai_runtime::logging::bound;
use boardai_runtime::session::{
    MemoryStore, SessionClose, SessionOpened, StoreLink, StoreRead, StoreSink,
};
use futures_util::future::BoxFuture;

/// Characters of a tool call or result printed; the rest is cut.
const PRINTED_DETAIL_MAX_BYTES: usize = 160;
/// Response text kept per answer for counting tags; past it the count is a lower bound.
const ANSWER_TEXT_MAX_BYTES: usize = 256 * 1024;

/// The numbers of one answer, from its first request to the orb going idle.
#[derive(Clone, Debug, Default)]
pub struct Answer {
    pub question: String,
    requested_at: Option<SessionTime>,
    pub first_token_ms: Option<u64>,
    pub first_audio_ms: Option<u64>,
    pub first_op_ms: Option<u64>,
    pub ops: u32,
    pub dropped_tags: u32,
    pub dropped_ops: u32,
    pub tool_calls: u32,
    pub interrupted: bool,
    text: String,
}

impl Answer {
    /// Tags the model wrote (`<name` of a known tag), whether or not they parsed.
    pub fn tags_written(&self) -> u32 {
        TAG_NAMES
            .iter()
            .map(|name| {
                let opening = format!("<{name}");
                let found = self
                    .text
                    .match_indices(&opening)
                    .filter(|(at, _)| {
                        let next = self.text[at + opening.len()..].chars().next();
                        next.is_none_or(|ch| ch.is_whitespace() || ch == '/' || ch == '>')
                    })
                    .count();
                u32::try_from(found).unwrap_or(u32::MAX)
            })
            .sum()
    }

    /// The drawing-rule numbers for everything the talker wrote in this answer.
    pub fn quality(&self) -> NarrationQuality {
        assess(&self.text)
    }
}

#[derive(Default)]
struct State {
    answers: Vec<Answer>,
    /// The answer being given; closed when the orb goes idle.
    open: Option<Answer>,
    /// The user's last finished words, for the next answer's label.
    heard: Option<String>,
    /// The current stream's raw narration, printed at its end with `--raw`.
    stream: String,
}

pub struct Printer {
    verbose: bool,
    raw: bool,
    memory: MemoryStore,
    state: Mutex<State>,
}

impl Printer {
    pub fn new(verbose: bool, raw: bool) -> Self {
        Self {
            verbose,
            raw,
            memory: MemoryStore::new(),
            state: Mutex::new(State::default()),
        }
    }

    /// Every finished answer, then the one still open.
    pub fn answers(&self) -> Vec<Answer> {
        let Ok(state) = self.state.lock() else {
            return Vec::new();
        };
        state.answers.iter().chain(&state.open).cloned().collect()
    }

    fn with_answer(&self, update: impl FnOnce(&mut Answer)) {
        if let Ok(mut state) = self.state.lock()
            && let Some(answer) = state.open.as_mut()
        {
            update(answer);
        }
    }

    fn on_milestone(&self, at: SessionTime, milestone: Milestone) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let state = &mut *state;
        if milestone == Milestone::TalkerRequested && state.heard.is_some() {
            // New words were heard since the last request: this is a new answer.
            if let Some(done) = state.open.take() {
                state.answers.push(done);
            }
            state.open = Some(Answer {
                question: state.heard.take().unwrap_or_default(),
                requested_at: Some(at),
                ..Answer::default()
            });
        }
        let Some(answer) = state.open.as_mut() else {
            line(at, "mark", &format!("{milestone:?}"));
            return;
        };
        let since = answer.requested_at.map(|requested| at.ms_since(requested));
        let slot = match milestone {
            Milestone::TalkerFirstToken => Some(&mut answer.first_token_ms),
            Milestone::FirstAudio => Some(&mut answer.first_audio_ms),
            Milestone::FirstOp => Some(&mut answer.first_op_ms),
            Milestone::BargeInStop => {
                answer.interrupted = true;
                None
            }
            Milestone::VoiceStart
            | Milestone::FinalTranscript
            | Milestone::TalkerRequested
            | Milestone::OnsetDuck
            | Milestone::BargeInDuck => None,
        };
        if let Some(slot) = slot
            && slot.is_none()
        {
            *slot = since;
        }
        let after = since.map_or_else(String::new, |ms| format!(" (+{ms} ms)"));
        line(at, "mark", &format!("{milestone:?}{after}"));
    }

    fn on_idle(&self, at: SessionTime) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let Some(answer) = state.open.take() else {
            return;
        };
        let quality = answer.quality();
        let summary = format!(
            "first token {} · first audio {} · first op {} · ops {} · tags {} written, {} dropped \
             · ops dropped {} · preamble {} words · missing words {} · tags on one word {} · \
             markup {}{}",
            ms(answer.first_token_ms),
            ms(answer.first_audio_ms),
            ms(answer.first_op_ms),
            answer.ops,
            answer.tags_written(),
            answer.dropped_tags,
            answer.dropped_ops,
            quality.preamble_words,
            quality.missing_words,
            quality.drawing_tags_per_word_max,
            quality.markup,
            if answer.interrupted {
                " · interrupted"
            } else {
                ""
            },
        );
        line(at, "done", &summary);
        state.answers.push(answer);
    }

    fn print_event(&self, at: SessionTime, event: &UiEvent) {
        match event {
            UiEvent::Caption {
                who,
                text,
                is_final,
            } => {
                if *is_final || self.verbose {
                    let label = match (who, is_final) {
                        (Speaker::User, _) => "you",
                        (Speaker::Assistant, true) => "say",
                        (Speaker::Assistant, false) => "say…",
                    };
                    line(at, label, text);
                }
            }
            UiEvent::OpStart {
                layer,
                op,
                budget_ms,
            } => {
                self.with_answer(|answer| answer.ops += 1);
                line(
                    at,
                    "op",
                    &format!("L{} {} budget {budget_ms} ms", layer.0, describe(op)),
                );
            }
            UiEvent::Freeze { layer } => line(at, "freeze", &format!("L{}", layer.0)),
            UiEvent::Resume { layer, budget_ms } => {
                line(at, "resume", &format!("L{} budget {budget_ms} ms", layer.0));
            }
            UiEvent::LayerPush { .. } | UiEvent::Thread { .. } | UiEvent::Camera { .. } => {
                board_line(at, event);
            }
            UiEvent::LayerPop { layer } => line(at, "detour", &format!("close L{}", layer.0)),
            UiEvent::SurfaceShow { surface } => line(at, "surface", &surface_line(surface)),
            UiEvent::SurfaceUpdate { id, update } => {
                line(at, "surface", &format!("#{} {update:?}", id.0));
            }
            UiEvent::SurfaceDismiss { id } => line(at, "surface", &format!("close #{}", id.0)),
            UiEvent::Chip { kind, text, source } => {
                let opens = source.as_ref().map_or_else(String::new, |source| {
                    format!(" (opens {}:{})", source.path, source.line)
                });
                line(at, "chip", &format!("{kind:?} {text}{opens}"));
            }
            UiEvent::Problem { spoken, detail } => {
                line(at, "problem", &format!("\"{spoken}\" ({detail})"));
            }
            UiEvent::Acknowledge => line(at, "ack", "backchannel heard; still speaking"),
            UiEvent::Suggestion { text } => {
                line(at, "suggest", text.as_deref().unwrap_or("(cleared)"));
            }
            UiEvent::SessionStart { .. }
            | UiEvent::ViewShow { .. }
            | UiEvent::ViewDismiss
            | UiEvent::OpenSettings => {
                print_session_event(at, event);
            }
            UiEvent::Orb { state, .. } => {
                if self.verbose {
                    line(at, "orb", &format!("{state:?}"));
                }
            }
        }
    }
}

impl StoreSink for Printer {
    fn report_to(&self, link: StoreLink) {
        self.memory.report_to(link);
    }

    fn input(&self, at: SessionTime, input: &Input) {
        match input {
            Input::TalkerDelta { text } => {
                if let Ok(mut state) = self.state.lock() {
                    if self.raw && state.stream.len() + text.len() <= ANSWER_TEXT_MAX_BYTES {
                        state.stream.push_str(text);
                    }
                    if let Some(answer) = state.open.as_mut()
                        && answer.text.len() + text.len() <= ANSWER_TEXT_MAX_BYTES
                    {
                        answer.text.push_str(text);
                    }
                }
            }
            Input::TalkerToolCall { invocation } => {
                self.with_answer(|answer| answer.tool_calls += 1);
                let mut detail = format!("{} {}", invocation.name, invocation.input_json);
                bound(&mut detail, PRINTED_DETAIL_MAX_BYTES);
                line(at, "tool", &detail);
            }
            Input::ToolResult { id, output } => {
                let mut detail = format!("{}: {}", id.as_str(), first_line(output));
                bound(&mut detail, PRINTED_DETAIL_MAX_BYTES);
                line(at, "result", &detail);
            }
            Input::Harness { task, event } => {
                let mut detail = format!("#{} {event:?}", task.0);
                bound(&mut detail, PRINTED_DETAIL_MAX_BYTES);
                line(at, "harness", &detail);
            }
            Input::TalkerDone { stop, .. } => {
                line(at, "stop", &format!("{stop:?}"));
                if let Ok(mut state) = self.state.lock()
                    && self.raw
                {
                    let text = std::mem::take(&mut state.stream);
                    println!("---- raw narration ----\n{text}\n-----------------------");
                }
            }
            Input::Transcript {
                text,
                is_final: true,
            } => {
                if let Ok(mut state) = self.state.lock() {
                    state.heard = Some(text.clone());
                }
            }
            Input::VoiceOnset
            | Input::VoiceStart
            | Input::VoiceEnd { .. }
            | Input::Transcript { .. }
            | Input::ClauseReady { .. }
            | Input::SynthesisFailed { .. }
            | Input::PlaybackAt { .. }
            | Input::PlaybackDrained
            | Input::VoiceStopped { .. }
            | Input::Command(_)
            | Input::Tick => {}
            Input::MemoryFailed { change, detail } => {
                let mut detail = format!("{change:?}: {detail}");
                bound(&mut detail, PRINTED_DETAIL_MAX_BYTES);
                line(at, "memory", &detail);
            }
        }
    }

    fn ui_event(&self, at: SessionTime, event: &UiEvent) {
        self.print_event(at, event);
        if let UiEvent::Orb {
            state: OrbState::Idle,
            ..
        } = event
        {
            self.on_idle(at);
        }
    }

    fn log(&self, at: SessionTime, record: &LogRecord) {
        match record {
            LogRecord::Milestone { milestone } => self.on_milestone(at, *milestone),
            LogRecord::Dropped { what, count } => {
                self.with_answer(|answer| match what {
                    Dropped::NarrationTag => answer.dropped_tags += count,
                    Dropped::Op => answer.dropped_ops += count,
                    Dropped::Detour => {}
                });
                line(at, "drop", &format!("{what:?} x{count}"));
            }
        }
    }

    fn remember(&self, at: SessionTime, memory: &MemoryWrite) {
        line(
            at,
            "memory",
            &format!("{:?} {:?} {}", memory.kind, memory.action, memory.text),
        );
        self.memory.remember(at, memory);
    }

    fn forget(&self, at: SessionTime, scope: ForgetScope) {
        line(at, "memory", &format!("forget {scope:?}"));
        self.memory.forget(at, scope);
    }

    fn accept_proposal(&self, at: SessionTime, capture: CaptureId) {
        line(at, "memory", &format!("keep proposal {}", capture.0));
        self.memory.accept_proposal(at, capture);
    }

    fn read(&self, read: StoreRead) -> BoxFuture<'static, ToolOutput> {
        self.memory.read(read)
    }

    fn start_session(&self) -> BoxFuture<'static, SessionOpened> {
        self.memory.start_session()
    }

    fn finish_session(&self, close: SessionClose) -> BoxFuture<'static, ()> {
        self.memory.finish_session(close)
    }

    fn flush(&self) -> BoxFuture<'static, ()> {
        self.memory.flush()
    }
}

/// A session starting, a view opening or closing, or the settings screen opening.
fn print_session_event(at: SessionTime, event: &UiEvent) {
    if let UiEvent::SessionStart { session } = event {
        line(at, "session", session.as_deref().unwrap_or("(memory off)"));
    } else if let UiEvent::ViewShow { view, .. } = event {
        line(at, "view", &format!("{:?}", view.kind()));
    } else if let UiEvent::ViewDismiss = event {
        line(at, "view", "closed");
    } else if let UiEvent::OpenSettings = event {
        line(at, "settings", "opened");
    }
}

fn line(at: SessionTime, label: &str, text: &str) {
    let seconds = at.as_ms() / 1_000;
    let millis = at.as_ms() % 1_000;
    println!("[{seconds:>4}.{millis:03}] {label:<8} {text}");
}

/// Detours, the thread and the camera, which take a few fields each.
fn board_line(at: SessionTime, event: &UiEvent) {
    if let UiEvent::LayerPush {
        layer,
        title,
        question,
    } = event
    {
        let question = question
            .as_ref()
            .map_or_else(String::new, |question| format!(" for \"{question}\""));
        line(
            at,
            "detour",
            &format!("open L{} \"{title}\"{question}", layer.0),
        );
    } else if let UiEvent::Thread {
        steps,
        current,
        detours,
    } = event
    {
        let current = current.map_or_else(|| "-".to_owned(), |step| step.to_string());
        let text = format!(
            "{} · step {current} · detours {detours:?}",
            steps.join(" > ")
        );
        line(at, "thread", &text);
    } else if let UiEvent::Camera {
        layer,
        frame,
        focus,
    } = event
    {
        let focus = focus.as_ref().map_or("-", |id| id.as_str());
        let text = format!(
            "L{} frame {}x{} at ({}, {}) focus {focus}",
            layer.0, frame.w, frame.h, frame.x, frame.y
        );
        line(at, "camera", &text);
    }
}

fn ms(value: Option<u64>) -> String {
    value.map_or_else(|| "-".to_owned(), |ms| format!("{ms} ms"))
}

fn describe(op: &DrawOp) -> String {
    match op {
        DrawOp::Node {
            id,
            op,
            shape,
            label,
            stack,
            ..
        } => {
            let stack = if *stack > 1 {
                format!(" x{stack}")
            } else {
                String::new()
            };
            format!(
                "#{} node {} {shape:?} \"{label}\"{stack}",
                op.0,
                id.as_str()
            )
        }
        DrawOp::Edge {
            op,
            from,
            to,
            label,
            style,
            ..
        } => {
            let label = label
                .as_ref()
                .map_or_else(String::new, |label| format!(" \"{label}\""));
            format!(
                "#{} edge {}>{} {style:?}{label}",
                op.0,
                from.as_str(),
                to.as_str()
            )
        }
        DrawOp::Annotate {
            op,
            target,
            mark,
            text,
            ..
        } => {
            let text = text
                .as_ref()
                .map_or_else(String::new, |text| format!(" \"{text}\""));
            format!("#{} annotate {} {mark:?}{text}", op.0, target.as_str())
        }
        DrawOp::Strike {
            op, target, label, ..
        } => {
            format!("#{} strike {} -> \"{label}\"", op.0, target.as_str())
        }
        DrawOp::Trace { op, id, points } => {
            format!("#{} trace {} ({} points)", op.0, id.as_str(), points.len())
        }
        DrawOp::Participant { .. }
        | DrawOp::Message { .. }
        | DrawOp::Cells { .. }
        | DrawOp::Write { .. } => describe_diagram(op),
    }
}

/// A sequence diagram's or a row of cells' op, in one line.
fn describe_diagram(op: &DrawOp) -> String {
    match op {
        DrawOp::Participant { op, id, label, .. } => {
            format!("#{} participant {} \"{label}\"", op.0, id.as_str())
        }
        DrawOp::Message {
            op,
            from,
            to,
            label,
            style,
            extend,
            ..
        } => {
            let label = label
                .as_ref()
                .map_or_else(String::new, |label| format!(" \"{label}\""));
            let grows = if extend.is_empty() {
                String::new()
            } else {
                format!(" (lifelines +{})", extend.len())
            };
            format!(
                "#{} message {}>{} {style:?}{label}{grows}",
                op.0,
                from.as_str(),
                to.as_str()
            )
        }
        DrawOp::Cells {
            op,
            id,
            cells,
            vertical,
            ..
        } => {
            let values: Vec<&str> = cells
                .iter()
                .map(|cell| {
                    if cell.value.is_empty() {
                        "_"
                    } else {
                        cell.value.as_str()
                    }
                })
                .collect();
            let column = if *vertical { " column" } else { "" };
            format!(
                "#{} cells {}[{}]{column}",
                op.0,
                id.as_str(),
                values.join("|")
            )
        }
        DrawOp::Write {
            op, target, label, ..
        } => format!("#{} write {} \"{label}\"", op.0, target.as_str()),
        DrawOp::Node { .. }
        | DrawOp::Edge { .. }
        | DrawOp::Annotate { .. }
        | DrawOp::Strike { .. }
        | DrawOp::Trace { .. } => String::new(),
    }
}

fn surface_line(surface: &Surface) -> String {
    match surface {
        Surface::Code {
            id,
            path,
            first_line,
            lines,
            ..
        } => {
            format!("#{} code {path}:{first_line} ({} lines)", id.0, lines.len())
        }
        Surface::Compare {
            id,
            options,
            criteria,
            title,
        } => {
            let title = title.as_deref().unwrap_or("compare");
            format!("#{} {title}: {options:?} by {criteria:?}", id.0)
        }
        Surface::Recap { id, bullets } => format!("#{} recap ({} bullets)", id.0, bullets.len()),
    }
}

fn first_line(output: &ToolOutput) -> String {
    match output {
        ToolOutput::File {
            path,
            first_line,
            lines,
            ..
        } => {
            format!("{path}:{first_line} ({} lines)", lines.len())
        }
        ToolOutput::Matches { hits, more_hits } => format!("{} hits (+{more_hits})", hits.len()),
        ToolOutput::Listing { entries, .. } => format!("{} entries", entries.len()),
        ToolOutput::Started { task } => format!("started task {}", task.0),
        ToolOutput::Text { text } => text.lines().next().unwrap_or_default().to_owned(),
        ToolOutput::Failed { kind, message } => format!("failed ({kind:?}): {message}"),
        ToolOutput::View { view, .. } => format!("view {:?}", view.kind()),
    }
}
