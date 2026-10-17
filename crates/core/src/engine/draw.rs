//! You draw too (artboard 13): BoardAI stops, says one fixed line, and watches while you draw.
//!
//! - **Entering.** "Let me draw" (a cue the core answers itself, like the memory controls), the
//!   pen button or `D`. BoardAI stops as for an interruption (the stroke in flight freezes, what
//!   was heard is committed), a change waiting for a yes and a proposed memory are closed, and it
//!   says [`DRAW_GO_AHEAD_LINE`] unless you are already talking or muted.
//! - **While you draw** no talker request starts (I14). The floor still moves as always (you
//!   speak, BoardAI's line can be cut off, you mute), so drawing sits beside the conversation
//!   state instead of being one. Each finished utterance is a label for your latest shape, a
//!   control ("undo that", "never mind", "done"), a question, or talk kept for the talker
//!   (`cues::drawing`). While you speak, words that could label your latest shape show a hint
//!   under it.
//! - **Done** ("done", a question, the Done button or Enter) describes your drawing for the
//!   talker and starts a turn with your words. **Esc** ("never mind") leaves without asking;
//!   your shapes stay and the talker sees them in the board summary.
//!
//! Your shapes are accepted whether or not you are drawing: one finished just as "done" was
//! heard still lands on the board.

use crate::board::user::{Added, Drawn, Undone, describe};
use crate::conversation::ConversationState;
use crate::conversation::cues::drawing::{self as cues, Heard, LabelForm};
use crate::conversation::cues::{MemoryCue, memory_cue};
use crate::effect::{Dropped, Effect, LogRecord, Milestone};
use crate::engine::gate::GateClosed;
use crate::engine::listen::PendingUser;
use crate::engine::stage::asks_to_end;
use crate::engine::text::DRAW_GO_AHEAD_LINE;
use crate::engine::{Core, TurnMarks, milestone};
use crate::ids::{ElementId, LayerId};
use crate::limits::{DRAWING_WORDS_MAX_BYTES, LABEL_MAX_BYTES, USER_SHAPES_PER_LAYER_MAX};
use crate::narration::tags::bounded;
use crate::protocol::{DrawAction, DrawnShape, LabelHint, UiEvent};
use crate::time::SessionTime;

/// Whether you are drawing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum Drawing {
    #[default]
    Off,
    On(Watch),
}

/// What BoardAI keeps while it watches you draw.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Watch {
    /// Your shapes of this drawing, oldest first: a label lands on the newest, and undo takes
    /// it. At most `USER_SHAPES_PER_LAYER_MAX`; past it the oldest can no longer be undone.
    shapes: Vec<(LayerId, ElementId)>,
    /// The shape of this drawing whose label was said alone ("cache"): a label said with a
    /// lead-in replaces it, since a remark is easily heard as a bare label.
    bare: Option<(LayerId, ElementId)>,
    /// What you said that was not a label or a control, for the talker, within
    /// `DRAWING_WORDS_MAX_BYTES`.
    words: String,
    /// `words` reached its bound: later words are dropped. A flag, not the "…" `bounded` ends
    /// a cut with: speech-to-text ends a trailing-off utterance with one too.
    full: bool,
    /// The label hint is on screen.
    hinting: bool,
}

impl Watch {
    fn newest(&self) -> Option<&(LayerId, ElementId)> {
        self.shapes.last()
    }

    /// Keeps what you said for the talker; once full, later words are dropped.
    fn keep(&mut self, text: &str) {
        let text = text.trim();
        if text.is_empty() || self.full {
            return;
        }
        let joined = if self.words.is_empty() {
            text.to_owned()
        } else {
            format!("{} {text}", self.words)
        };
        self.full = joined.len() > DRAWING_WORDS_MAX_BYTES;
        self.words = bounded(&joined, DRAWING_WORDS_MAX_BYTES);
    }
}

fn dropped(effects: &mut Vec<Effect>, what: Dropped) {
    effects.push(Effect::Log(LogRecord::Dropped { what, count: 1 }));
}

impl Core {
    /// Whether you are drawing: BoardAI watches, and no talker request starts.
    #[must_use]
    pub fn is_drawing(&self) -> bool {
        matches!(self.drawing, Drawing::On(_))
    }

    pub(super) fn on_draw(
        &mut self,
        now: SessionTime,
        action: DrawAction,
        effects: &mut Vec<Effect>,
    ) {
        match action {
            DrawAction::Start => self.start_drawing(now, String::new(), effects),
            DrawAction::Shape { layer, drawn } => self.add_shape(layer, drawn, effects),
            DrawAction::Undo => self.undo_shape(effects),
            DrawAction::Done => self.done_drawing(now, effects),
            DrawAction::Cancel => self.cancel_drawing(now, effects),
        }
    }

    /// "Let me draw what I mean": answered here, with no talker request (returns true).
    pub(super) fn draw_by_voice(
        &mut self,
        now: SessionTime,
        text: &str,
        effects: &mut Vec<Effect>,
    ) -> bool {
        if !cues::is_start(text) {
            return false;
        }
        self.start_drawing(now, text.to_owned(), effects);
        true
    }

    /// You start drawing; `said` is what you said to ask for it, kept for the talker.
    fn start_drawing(&mut self, now: SessionTime, said: String, effects: &mut Vec<Effect>) {
        if self.is_drawing() {
            return;
        }
        // Drawing isn't an answer: a change or a memory waiting for a yes is closed. A view read
        // for a tag of the reply would open over the drawing: it is dropped.
        self.close_change_gate(GateClosed::Drawing);
        self.close_proposal_for_drawing();
        self.drop_written_reads();
        self.clear_suggestion(effects);
        let speak = match std::mem::replace(&mut self.state, ConversationState::Idle) {
            // An interruption with no words: what was heard is committed, the stroke freezes.
            ConversationState::Speaking | ConversationState::Thinking => {
                milestone(effects, Milestone::BargeInStop);
                self.user.context.interrupted = true;
                self.stop_reply(now, false, effects);
                true
            }
            // You are already talking over BoardAI: that confirms your interruption, and your
            // words are the drawing's first.
            ConversationState::Ducked(barge_in) => {
                self.interrupt(now, barge_in.into_utterance(), effects);
                false
            }
            state @ (ConversationState::Listening(_) | ConversationState::Muted) => {
                self.state = state;
                false
            }
            ConversationState::Idle => true,
        };
        if self.stop.is_some() {
            self.settle_stop(None, effects);
        }
        let mut watch = Watch::default();
        watch.keep(&said);
        self.drawing = Drawing::On(watch);
        effects.push(Effect::Ui(UiEvent::DrawMode { drawing: true }));
        if speak {
            self.say(DRAW_GO_AHEAD_LINE, effects);
        }
    }

    /// A shape you finished. It joins the latest pen shape of this drawing when it is a pen
    /// stroke mostly over it.
    fn add_shape(&mut self, layer: LayerId, drawn: DrawnShape, effects: &mut Vec<Effect>) {
        let join = match &self.drawing {
            Drawing::On(watch) => watch
                .newest()
                .filter(|(on, _)| *on == layer)
                .map(|(_, id)| id.clone()),
            Drawing::Off => None,
        };
        let added = Drawn::parse(drawn).and_then(|drawn| {
            self.board
                .add_user_shape_connecting(layer, drawn, join.as_ref())
        });
        let Ok(Added { shape, connected }) = added else {
            dropped(effects, Dropped::UserShape);
            return;
        };
        if let Drawing::On(watch) = &mut self.drawing
            && watch.newest().map(|(_, id)| id) != Some(&shape.id)
        {
            if watch.shapes.len() >= USER_SHAPES_PER_LAYER_MAX {
                watch.shapes.remove(0);
            }
            watch.shapes.push((layer, shape.id.clone()));
        }
        effects.push(Effect::Ui(UiEvent::UserShape { layer, shape }));
        for arrow in connected {
            effects.push(Effect::Ui(UiEvent::UserShape {
                layer,
                shape: arrow,
            }));
        }
    }

    /// Undo: the newest shape of this drawing goes (a pen shape loses its last stroke).
    fn undo_shape(&mut self, effects: &mut Vec<Effect>) {
        let Drawing::On(watch) = &mut self.drawing else {
            dropped(effects, Dropped::UserShape);
            return;
        };
        let Some((layer, id)) = watch.shapes.pop() else {
            dropped(effects, Dropped::UserShape);
            return;
        };
        match self.board.undo_user_shape(layer, &id) {
            Some(Undone::Removed { disconnected }) => {
                effects.push(Effect::Ui(UiEvent::UserShapeRemove { layer, id }));
                for arrow in disconnected {
                    effects.push(Effect::Ui(UiEvent::UserShape {
                        layer,
                        shape: arrow,
                    }));
                }
            }
            Some(Undone::Changed(shape)) => {
                watch.shapes.push((layer, id));
                effects.push(Effect::Ui(UiEvent::UserShape { layer, shape }));
            }
            None => {
                watch.shapes.push((layer, id));
                dropped(effects, Dropped::UserShape);
            }
        }
    }

    /// Done or Enter. Words you were saying count as said at the end; BoardAI's line stops.
    /// Muted, nothing happens: BoardAI can't answer until you unmute.
    fn done_drawing(&mut self, now: SessionTime, effects: &mut Vec<Effect>) {
        if !self.is_drawing() || self.state == ConversationState::Muted {
            dropped(effects, Dropped::DrawCommand);
            return;
        }
        let said = match std::mem::replace(&mut self.state, ConversationState::Idle) {
            ConversationState::Listening(utterance) => utterance.text(),
            ConversationState::Ducked(barge_in) => {
                self.stop_reply(now, false, effects);
                barge_in.into_utterance().text()
            }
            ConversationState::Speaking | ConversationState::Thinking => {
                self.stop_reply(now, false, effects);
                String::new()
            }
            ConversationState::Idle | ConversationState::Muted => String::new(),
        };
        if self.stop.is_some() {
            self.settle_stop(None, effects);
        }
        self.finish_drawing(now, &said, effects);
    }

    /// Esc or "never mind": you stop drawing without asking anything. Your shapes stay, and the
    /// talker hears they were not asked about. BoardAI's line, if it is still saying it, stops,
    /// and a question nothing was heard of (you pressed `D` while BoardAI thought) is dropped
    /// whether or not the line is still playing, as Esc drops one.
    fn cancel_drawing(&mut self, now: SessionTime, effects: &mut Vec<Effect>) {
        let Drawing::On(watch) = std::mem::take(&mut self.drawing) else {
            dropped(effects, Dropped::DrawCommand);
            return;
        };
        self.end_watch(&watch, effects);
        self.user.pending = None;
        if matches!(
            self.state,
            ConversationState::Speaking | ConversationState::Thinking
        ) {
            self.state = ConversationState::Idle;
            self.stop_reply(now, true, effects);
        }
        if !watch.shapes.is_empty() {
            self.context.notice(
                "The user drew on the board (\"drawn by the user\" in the board summary) and \
                 stopped drawing without asking you about it."
                    .to_owned(),
            );
        }
    }

    /// Your utterance ended while you draw: it labels, controls, finishes, or is kept. Returns
    /// false when you are not drawing, and the turn goes on as usual.
    pub(super) fn drawing_heard(
        &mut self,
        now: SessionTime,
        text: &str,
        effects: &mut Vec<Effect>,
    ) -> bool {
        if !self.is_drawing() {
            return false;
        }
        self.hint(None, effects);
        // What means the same while you draw is answered as always: ending the session (alone or
        // joined to other controls), opening settings, going off the record. None of them asks
        // the talker.
        let off_record = matches!(memory_cue(text), Some(MemoryCue::ForgetSession));
        if self.end_session_by_voice(text, effects)
            || self.open_settings_by_voice(text, effects)
            || (off_record && self.memory_control(text, effects))
            || (asks_to_end(text) && self.joined_controls(text, effects))
        {
            return true;
        }
        let meaning = cues::heard(text);
        // "Stop that research" stops it now, as always, and the talker hears the words when you
        // finish. A drawing control wins: "cancel that" leaves the drawing, never a task.
        if meaning == Heard::Talk {
            self.cancel_by_voice(text, effects);
        }
        match meaning {
            Heard::Nothing => {}
            Heard::Cancel => self.cancel_drawing(now, effects),
            Heard::Undo => self.undo_shape(effects),
            Heard::Done { before } => {
                match cues::label_or_talk(before) {
                    Heard::Label { text, form } => self.label_newest(&text, form, before, effects),
                    Heard::Nothing
                    | Heard::Cancel
                    | Heard::Undo
                    | Heard::Done { .. }
                    | Heard::Question
                    | Heard::Talk => self.keep_words(before),
                }
                self.finish_drawing(now, "", effects);
            }
            Heard::Question => self.finish_drawing(now, text, effects),
            Heard::Label { text: label, form } => self.label_newest(&label, form, text, effects),
            Heard::Talk => self.keep_words(text),
        }
        true
    }

    /// The words label your newest shape, or become its note once it has a label; a label said
    /// with a lead-in replaces one said alone. With no shape yet they are kept as talk.
    fn label_newest(
        &mut self,
        label: &str,
        form: LabelForm,
        said: &str,
        effects: &mut Vec<Effect>,
    ) {
        let Drawing::On(watch) = &mut self.drawing else {
            return;
        };
        let Some(newest) = watch.newest().cloned() else {
            self.keep_words(said);
            return;
        };
        let replaces = form == LabelForm::LeadIn && watch.bare.as_ref() == Some(&newest);
        let (layer, id) = newest;
        let labelled = if replaces {
            self.board.relabel_user_shape(layer, &id, label)
        } else {
            self.board.label_user_shape(layer, &id, label)
        };
        let Some(shape) = labelled else {
            self.keep_words(said);
            return;
        };
        if let Drawing::On(watch) = &mut self.drawing {
            let alone = form == LabelForm::Alone && shape.note.is_none();
            watch.bare = alone.then(|| (layer, id.clone()));
        }
        effects.push(Effect::Ui(UiEvent::UserShape { layer, shape }));
    }

    fn keep_words(&mut self, text: &str) {
        if let Drawing::On(watch) = &mut self.drawing {
            watch.keep(text);
        }
    }

    /// While you speak: words that could label your newest shape show the hint under it.
    pub(super) fn hint_label(&mut self, words: &str, effects: &mut Vec<Effect>) {
        let Drawing::On(watch) = &self.drawing else {
            return;
        };
        let hint = watch
            .newest()
            .filter(|_| cues::could_label(words))
            .map(|(_, target)| LabelHint {
                target: target.clone(),
                words: bounded(words, LABEL_MAX_BYTES),
            });
        self.hint(hint, effects);
    }

    /// Clears the hint once you are no longer speaking (muted, cut off, or answered).
    pub(super) fn settle_hint(&mut self, effects: &mut Vec<Effect>) {
        if !matches!(
            self.state,
            ConversationState::Listening(_) | ConversationState::Ducked(_)
        ) {
            self.hint(None, effects);
        }
    }

    fn hint(&mut self, hint: Option<LabelHint>, effects: &mut Vec<Effect>) {
        let Drawing::On(watch) = &mut self.drawing else {
            return;
        };
        if hint.is_none() && !watch.hinting {
            return;
        }
        watch.hinting = hint.is_some();
        effects.push(Effect::Ui(UiEvent::LabelHint { hint }));
    }

    fn end_watch(&mut self, watch: &Watch, effects: &mut Vec<Effect>) {
        if watch.hinting {
            effects.push(Effect::Ui(UiEvent::LabelHint { hint: None }));
        }
        effects.push(Effect::Ui(UiEvent::DrawMode { drawing: false }));
    }

    /// Your drawing is finished: the talker gets it described, with your words, in a new turn.
    /// `said` is what you said as you finished ("does this make sense?").
    fn finish_drawing(&mut self, now: SessionTime, said: &str, effects: &mut Vec<Effect>) {
        let Drawing::On(mut watch) = std::mem::take(&mut self.drawing) else {
            return;
        };
        self.end_watch(&watch, effects);
        watch.keep(said);
        let mut context = std::mem::take(&mut self.user.context);
        context.drawing = self.drawing_block();
        // Shapes from earlier drawings are on the board too: only this one's count as drawn.
        let drew = !watch.shapes.is_empty();
        let mut text = watch.words;
        // A question nothing of whose answer was heard (you pressed D while BoardAI thought)
        // is the same turn.
        if let Some(previous) = self.user.pending.take() {
            text = format!("{} {text}", previous.text).trim().to_owned();
            let mut merged = previous.context;
            merged.merge(context);
            context = merged;
        }
        if text.trim().is_empty() && !drew {
            self.user.context = context;
            return;
        }
        self.turn_question = self.question_of(said, &context);
        let message = self.user_message(&text, &context);
        self.user.pending = Some(PendingUser {
            text,
            context,
            message,
        });
        self.marks = TurnMarks::default();
        self.state = ConversationState::Thinking;
        self.request_talk(now, effects);
    }

    /// Your shapes on every open layer, described for the talker; `None` when there are none.
    fn drawing_block(&self) -> Option<String> {
        let lines: Vec<String> = self
            .board
            .layers()
            .iter()
            .filter(|layer| layer.is_open())
            .filter_map(|layer| {
                let drawing = describe(layer)?;
                Some(if layer.id() == LayerId::MAIN {
                    format!("[The user drew on the board: {drawing}]")
                } else {
                    format!(
                        "[The user drew on the detour \"{}\": {drawing}]",
                        layer.title()
                    )
                })
            })
            .collect();
        (!lines.is_empty()).then(|| lines.join("\n"))
    }
}
