//! Drill-down (D-188): a detail layer anchored to a part of the board. The camera moves into the
//! part and the layer fills the board's area (artboard 15b); breadcrumbs name the path back. A
//! detail layer is a detour (I4): it is pushed and popped on the thread's stack, at most
//! `DETOUR_DEPTH_MAX` deep below the main board, so a map drills four levels at most.
//!
//! - **By tag**: `<detail of="arena" title="…">…</detail>` opens the layer when its words play
//!   and stages the diagram it holds there. A detail already open on that part (opened by voice)
//!   takes the diagram instead of opening another.
//! - **By voice**: "more detail on the arena" or "zoom into the arena" opens the layer at once,
//!   matched against what is on the board, and the talker is asked to fill it. Naming nothing on
//!   the board, the words are the talker's as they are.
//! - **Back out**: "back out" or Esc (when BoardAI is quiet) closes the innermost layer and the
//!   stroke frozen under it finishes, as `<resume/>` does. Alone, the core answers it; with a
//!   request, the talker hears the rest on the layer below.

use crate::board::{ElementState, Layer};
use crate::conversation::detail_cues::{back_out_cue, detail_cue, name_words};
use crate::conversation::stage_cues::PageCue;
use crate::effect::Effect;
use crate::engine::Core;
use crate::geometry::Rect;
use crate::ids::{ElementId, LayerId, OpId};
use crate::limits::DETOUR_DEPTH_MAX;
use crate::narration::diagram::DiagramSpec;
use crate::protocol::{Category, DetailAnchor, UiEvent};
use crate::time::SessionTime;

impl Core {
    /// `<detail of="…">`, as its words play: the layer opens anchored to `of`, with the diagram
    /// it holds staged on it.
    pub(super) fn open_detail(
        &mut self,
        op: OpId,
        of: &ElementId,
        title: Option<&str>,
        spec: Option<&DiagramSpec>,
        effects: &mut Vec<Effect>,
    ) -> bool {
        let open_on_it = self
            .thread
            .detours()
            .top()
            .and_then(|detour| detour.anchor.as_ref())
            .is_some_and(|anchor| anchor.element == *of);
        if !open_on_it && self.push_detail(of, title, effects).is_none() {
            return false;
        }
        match spec {
            Some(spec) => self.stage_diagram(op, spec, effects),
            None => true,
        }
    }

    /// Opens a detail layer anchored to `of` on the active layer.
    pub(super) fn push_detail(
        &mut self,
        of: &ElementId,
        title: Option<&str>,
        effects: &mut Vec<Effect>,
    ) -> Option<LayerId> {
        let under = self.thread.active_layer();
        let (anchor, label) = self.detail_anchor(under, of);
        let title = title.map_or(label, str::to_owned);
        let mut events = Vec::new();
        let layer = self.push_detour_layer(&title, Some(anchor), &mut events, effects);
        effects.extend(events.into_iter().map(Effect::Ui));
        layer
    }

    /// Where a detail on `of` drills in, and the part's label. A name that isn't on the layer
    /// drills into the whole drawing, so the explanation is never lost.
    fn detail_anchor(&self, layer: LayerId, of: &ElementId) -> (DetailAnchor, String) {
        let board_layer = self.board.layer(layer);
        let element = board_layer
            .and_then(|board_layer| board_layer.element(of))
            .filter(|element| element.state.is_shown() || self.board.is_planned(layer, of));
        let rect = element.map_or_else(
            || board_layer.and_then(Layer::bounds).unwrap_or_default(),
            crate::board::Element::area,
        );
        let label = element.map_or_else(|| of.as_str().to_owned(), |element| element.label.clone());
        let anchor = DetailAnchor {
            layer,
            element: of.clone(),
            rect,
            category: self.tone_of(layer, of),
        };
        (anchor, label)
    }

    /// The category a staged part is drawn in.
    fn tone_of(&self, layer: LayerId, id: &ElementId) -> Option<Category> {
        self.board.staged(layer).iter().find_map(|diagram| {
            let entry = diagram.entry_of(id)?;
            diagram.entries[entry].tone
        })
    }

    /// A `<detour>` the talker writes into the detail layer the user just opened by voice, before
    /// anything is drawn there, is that layer: the talker answers "more detail on X" with a
    /// detour as often as with a detail, and a second layer would stack an empty one under it.
    pub(super) fn absorbs_detour(&mut self) -> bool {
        let Some(layer) = self.staging.voice_detail else {
            return false;
        };
        let empty = self.thread.active_layer() == layer
            && self
                .board
                .layer(layer)
                .is_some_and(|board| board.elements().is_empty());
        if empty {
            self.staging.voice_detail = None;
        }
        empty
    }

    /// "Back out", or Esc while BoardAI is quiet: the innermost detail or detour closes and the
    /// stroke frozen under it finishes. False with nothing open.
    pub(super) fn back_out(&mut self, now: SessionTime, effects: &mut Vec<Effect>) -> bool {
        if self.thread.depth() == 0 {
            return false;
        }
        let mut events = Vec::new();
        self.close_detour(&mut events);
        effects.extend(events.into_iter().map(Effect::Ui));
        let layer = self.thread.active_layer();
        self.resume_frozen(now, layer, effects);
        true
    }

    /// "Back out": alone, answered here with a pulse (returns true); leading or ending a
    /// request, the layer closes and the talker hears the rest with a note.
    pub(super) fn back_out_by_voice(
        &mut self,
        now: SessionTime,
        text: &str,
        effects: &mut Vec<Effect>,
    ) -> bool {
        let Some(cue) = back_out_cue(text) else {
            return false;
        };
        let title = self
            .thread
            .detours()
            .top()
            .map(|detour| detour.title.clone());
        if !self.back_out(now, effects) {
            return false;
        }
        let closed = title.unwrap_or_default();
        match cue {
            PageCue::Only => {
                effects.push(Effect::Ui(UiEvent::Acknowledge));
                self.context.notice(format!(
                    "The user said \"{text}\": the layer \"{closed}\" closed, and they are back on \
                     the one under it. Wait for their next question."
                ));
                true
            }
            PageCue::WithRequest => {
                self.context.notice(format!(
                    "The user said \"{text}\", so the layer \"{closed}\" closed. Answer the rest of \
                     their words on the layer under it, and don't write <resume/>."
                ));
                false
            }
        }
    }

    /// "More detail on the arena": the detail layer opens anchored to what the words name, and
    /// the talker is told to fill it. True when a layer opened.
    pub(super) fn detail_by_voice(&mut self, text: &str, effects: &mut Vec<Effect>) -> bool {
        let Some(cue) = detail_cue(text) else {
            return false;
        };
        let layer = self.thread.active_layer();
        let Some((id, label)) = self.named_part(layer, &cue.target) else {
            return false;
        };
        if self.thread.depth() >= DETOUR_DEPTH_MAX {
            self.context.notice(format!(
                "The user asked for more detail on {} \"{label}\", but the board is already \
                 {DETOUR_DEPTH_MAX} levels deep; answer here, or suggest backing out first.",
                id.as_str()
            ));
            return false;
        }
        let Some(opened) = self.push_detail(&id, None, effects) else {
            return false;
        };
        self.staging.voice_detail = Some(opened);
        let fill = match self.brief_coming_for(id.as_str(), &label) {
            // A shared source's brief on this part is on its way: its diagram is staged here
            // when it arrives (D-242), so the talker doesn't draw a guess under it.
            Some(harness) => format!(
                "{harness} is reading it, and its brief's diagram will be staged on this layer                  when it arrives. Say so in a few words; don't draw its internals yourself."
            ),
            None => "Draw its internals there at once: one <diagram> block with its parts and                      their fields, then <reveal> them as you explain."
                .to_owned(),
        };
        self.context.notice(format!(
            "The user said \"{text}\", so a detail layer anchored to {} \"{label}\" is open and \
             fills the board. {fill} Write <resume/> only when you are done with it; the user can \
             also say \"back out\".",
            id.as_str()
        ));
        true
    }

    /// The part of `layer` the spoken words name: a drawn element whose label or id has those
    /// words, the closest match first (containers before the parts inside them, by size).
    fn named_part(&self, layer: LayerId, target: &str) -> Option<(ElementId, String)> {
        let wanted = name_words(target);
        if wanted.is_empty() {
            return None;
        }
        let board_layer = self.board.layer(layer)?;
        board_layer
            .elements()
            .iter()
            .filter(|element| {
                element.op.is_some()
                    && (element.state.is_shown() || element.state == ElementState::Planned)
            })
            .filter_map(|element| {
                let score = match_score(&wanted, &element.label)
                    .max(match_score(&wanted, element.id.as_str()))?;
                let area = element.area();
                Some((
                    score,
                    area_of(area),
                    element.id.clone(),
                    element.label.clone(),
                ))
            })
            .max_by_key(|(score, area, _, _)| (*score, *area))
            .map(|(_, _, id, label)| (id, label))
    }
}

/// How well `wanted` names `name`: 3 for the same words, 2 when one holds the other's words
/// in a row, else none.
fn match_score(wanted: &[String], name: &str) -> Option<u8> {
    let words = name_words(name);
    if words.is_empty() {
        return None;
    }
    if words == wanted {
        return Some(3);
    }
    let holds = |outer: &[String], inner: &[String]| {
        inner.len() <= outer.len() && outer.windows(inner.len()).any(|window| window == inner)
    };
    (holds(&words, wanted) || holds(wanted, &words)).then_some(2)
}

fn area_of(rect: Rect) -> u64 {
    u64::from(rect.w) * u64::from(rect.h)
}
