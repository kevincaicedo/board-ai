//! A research brief's diagram on the board (D-242). A brief's `## Diagram` is a `<diagram>`
//! block in the narration vocabulary (D-234). The core reads it with the narration's own parser
//! as the brief arrives, and stages it (D-235, D-236) when the brief reaches the talker.
//!
//! - **When.** With the request that carries the brief: a held `research` call's result, or the
//!   brief's report turn. A prefetch goes quietly with whatever is asked next, so its diagram
//!   waits until the talker uses the brief (a `research` call about it): a map nobody asked about
//!   never lands under another question. No request goes out while the user draws (I14) or once
//!   the session ended, so no staging does either.
//! - **Where.** On the layer in front: the main board for an overview (a full page turns first,
//!   D-177); for a component brief, the detail layer anchored to that part, opened on it now
//!   when the board in front has it (D-238).
//! - **What the talker hears.** A note with the staged ids. It reveals them as it explains
//!   (D-237), and what its reply leaves planned is revealed as the reply ends.
//! - **Untrusted.** The block is harness output (D-107): staging lays it out and draws it, nothing
//!   in it runs, and every tag or part that doesn't parse or fit is dropped and counted.
//! - **Provenance.** The staged diagram names who read the code, the source and the files read;
//!   the header shows it as a chip ("from Claude Code's brief · 23 files read").

use std::fmt::Write as _;

use crate::board::ElementState;
use crate::effect::{Dropped, Effect, LogRecord};
use crate::engine::Core;
use crate::engine::ops::{Anchor, QueuedOp};
use crate::engine::reveal::{StagedHere, not_staged_because};
use crate::engine::shared::BriefTask;
use crate::ids::{ElementId, LayerId, OpId};
use crate::limits::{BRIEF_DIAGRAMS_WAITING_MAX, DETOUR_DEPTH_MAX, TITLE_MAX_BYTES};
use crate::narration::diagram::{DiagramSpec, WrittenDiagram};
use crate::narration::ops::NarrationOp;
use crate::narration::tags::bounded;
use crate::protocol::{DiagramProvenance, StagedPart};
use crate::sources::brief::Brief;
use crate::sources::{BriefTopic, SourceId, name_key};

/// Parts named in the note to the talker; the rest are counted.
const NOTE_PARTS_MAX: usize = 24;

/// When a brief's diagram goes on the board.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::engine) enum Carried {
    /// With the next talker request: it carries the brief (a held call's result, a report).
    NextRequest,
    /// Once the talker uses the brief: a prefetch, which goes quietly with the next question.
    OnUse,
}

/// What a brief's report tells the talker about its diagram.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::engine) enum BriefDiagram {
    None,
    Staged(Carried),
    /// It had one, and nothing in it could be read.
    Unreadable,
}

/// A brief's diagram, read, waiting for its brief to reach the talker.
#[derive(Clone, Debug)]
pub(in crate::engine) struct WaitingDiagram {
    source: SourceId,
    topic: BriefTopic,
    /// How the talker knows the brief: "Claude Code's brief on the overview of inf-alloc".
    brief: String,
    spec: DiagramSpec,
    /// Tags the narration's parser dropped from it.
    dropped: u32,
    provenance: DiagramProvenance,
    carried: Carried,
}

impl Core {
    /// Reads a brief's diagram as the brief arrives; it waits to be staged with the brief.
    /// Returns what the report says about it.
    pub(in crate::engine) fn wait_to_stage(
        &mut self,
        brief_task: &BriefTask,
        brief: &Brief,
        cached: bool,
        carried: Carried,
        effects: &mut Vec<Effect>,
    ) -> BriefDiagram {
        let Some(diagram) = &brief.diagram else {
            return BriefDiagram::None;
        };
        let read = WrittenDiagram::read(&diagram.text);
        if read.dropped > 0 {
            effects.push(Effect::Log(LogRecord::Dropped {
                what: Dropped::DiagramPart,
                count: read.dropped,
            }));
        }
        let Some(mut spec) = read.spec else {
            return BriefDiagram::Unreadable;
        };
        let source = brief_task.source;
        let title = self
            .sources
            .shared
            .get(source)
            .map(|entry| entry.info.title.clone())
            .unwrap_or_default();
        let harness = self.sources.shared.harness_name().to_owned();
        if spec.title == spec.id.as_str() {
            // No title of its own: named after what it maps.
            spec.title = match &brief_task.topic {
                BriefTopic::Overview => bounded(&title, TITLE_MAX_BYTES),
                BriefTopic::Component { name } => {
                    bounded(&format!("{title} · {name}"), TITLE_MAX_BYTES)
                }
            };
        }
        let waiting = WaitingDiagram {
            source,
            topic: brief_task.topic.clone(),
            brief: format!(
                "{harness}'s brief on {} of {title}",
                brief_task.topic.spoken()
            ),
            spec,
            dropped: read.dropped,
            provenance: DiagramProvenance {
                reader: harness,
                source: title,
                files_read: (!cached).then_some(brief_task.files_read),
            },
            carried,
        };
        self.keep_waiting(waiting, effects);
        BriefDiagram::Staged(carried)
    }

    /// One diagram per source and topic, the newest; past the bound the oldest is forgotten.
    fn keep_waiting(&mut self, waiting: WaitingDiagram, effects: &mut Vec<Effect>) {
        let diagrams = &mut self.sources.diagrams;
        diagrams.retain(|known| !(known.source == waiting.source && known.topic == waiting.topic));
        diagrams.push_back(waiting);
        while diagrams.len() > BRIEF_DIAGRAMS_WAITING_MAX {
            diagrams.pop_front();
            effects.push(Effect::Log(LogRecord::Dropped {
                what: Dropped::DiagramPart,
                count: 1,
            }));
        }
    }

    /// A source no longer shared (removed, or its project left): its diagrams wait for nothing.
    pub(in crate::engine) fn forget_diagrams_of(&mut self, sources: &[SourceId]) {
        self.sources
            .diagrams
            .retain(|waiting| !sources.contains(&waiting.source));
    }

    /// The talker uses a brief it already has (a `research` call about it): its diagram, if one
    /// waits for that, goes on the board with this request. True when one did.
    pub(in crate::engine) fn diagram_in_use(
        &mut self,
        source: SourceId,
        topic: &BriefTopic,
    ) -> bool {
        let waiting = self
            .sources
            .diagrams
            .iter_mut()
            .find(|waiting| waiting.source == source && waiting.topic == *topic);
        match waiting {
            Some(waiting) => {
                waiting.carried = Carried::NextRequest;
                true
            }
            None => false,
        }
    }

    /// A talker request is going out: the briefs' diagrams it carries are staged first, so the
    /// board summary and a note name their ids. A new turn starts with no brief diagram of its
    /// own: what an earlier reply left planned stays so, and the summary says what is planned.
    pub(in crate::engine) fn stage_brief_diagrams(
        &mut self,
        new_turn: bool,
        effects: &mut Vec<Effect>,
    ) {
        if new_turn {
            let _ = self.staging.take_briefs();
        }
        let mut due = Vec::new();
        self.sources.diagrams.retain(|waiting| {
            let now = waiting.carried == Carried::NextRequest;
            if now {
                due.push(waiting.clone());
            }
            !now
        });
        for waiting in due {
            self.stage_brief(waiting, effects);
        }
    }

    fn stage_brief(&mut self, waiting: WaitingDiagram, effects: &mut Vec<Effect>) {
        let detail = match &waiting.topic {
            BriefTopic::Overview => None,
            BriefTopic::Component { name } => self.detail_for(waiting.source, name, effects),
        };
        let op = self.fresh_op();
        let staged = self.stage_here(op, &waiting.spec, Some(waiting.provenance.clone()), effects);
        match staged {
            Ok(here) => {
                self.staging
                    .brief_staged(here.layer, here.staged.id.clone());
                let note = self.staged_note(&waiting, &here, detail.as_deref());
                self.context.notice(note);
            }
            Err(refused) => self.context.notice(format!(
                "The diagram of {} was not staged: {}. Draw what you explain with your own \
                 tags, a part at a time.",
                waiting.brief,
                not_staged_because(&refused)
            )),
        }
    }

    /// What the talker is told about a brief's staged diagram: where it is, its ids, and how to
    /// reveal them.
    fn staged_note(
        &self,
        waiting: &WaitingDiagram,
        here: &StagedHere,
        detail: Option<&str>,
    ) -> String {
        let staged = &here.staged;
        let place = match detail {
            Some(label) => {
                format!("on the detail layer anchored to \"{label}\", open in front of the board")
            }
            None => "on the board".to_owned(),
        };
        let mut note = format!(
            "The diagram of {} is staged {place} (id {}, \"{}\"); nothing of it is drawn yet.",
            waiting.brief,
            staged.id.as_str(),
            staged.title
        );
        if here.page_turned {
            note.push_str(" The page turned to make room for it.");
        }
        let parts = self.part_tokens(here.layer, &staged.id);
        let _ = write!(note, " Its parts: {}.", parts.join(", "));
        note.push_str(
            " Reveal each with <reveal id=\"…\"/> right before the words that explain it \
             (deep=\"true\" on a container brings what it holds), and don't write it again as a \
             <diagram>. Whatever you haven't revealed when your reply ends is drawn then.",
        );
        let dropped = waiting.dropped.saturating_add(staged.dropped);
        if dropped > 0 {
            let _ = write!(
                note,
                " {dropped} of its tags or parts were dropped (malformed, misplaced, a repeated \
                 or taken id, or a connector naming nothing)."
            );
        }
        note
    }

    /// `id "label"` for each part of diagram `id` on `layer` (not its connectors or notes), at
    /// most [`NOTE_PARTS_MAX`], then how many more.
    fn part_tokens(&self, layer: LayerId, id: &ElementId) -> Vec<String> {
        let Some(diagram) = self
            .board
            .staged(layer)
            .iter()
            .find(|diagram| diagram.id == *id)
        else {
            return Vec::new();
        };
        let parts: Vec<String> = diagram
            .entries
            .iter()
            .filter(|entry| entry.ends.is_none() && !matches!(entry.draw, StagedPart::Note { .. }))
            .map(|entry| match entry.draw.label() {
                Some(label) if !label.is_empty() && label != entry.id.as_str() => {
                    format!("{} \"{label}\"", entry.id.as_str())
                }
                Some(_) | None => entry.id.as_str().to_owned(),
            })
            .collect();
        let more = parts.len().saturating_sub(NOTE_PARTS_MAX);
        let mut named: Vec<String> = parts.into_iter().take(NOTE_PARTS_MAX).collect();
        if more > 0 {
            named.push(format!("and {more} more"));
        }
        named
    }

    /// The detail layer a component brief's diagram goes on: the one in front when it is
    /// anchored to that part; else one opened on it now when the board in front has it (D-238,
    /// within the detour depth). Returns the part's label; `None` stages on the layer in front.
    fn detail_for(
        &mut self,
        source: SourceId,
        name: &str,
        effects: &mut Vec<Effect>,
    ) -> Option<String> {
        let mut names = vec![name_key(name)];
        if let Some((id, label)) = self
            .sources
            .shared
            .get(source)
            .and_then(|entry| entry.component(name))
        {
            names.push(name_key(id));
            names.push(name_key(label));
        }
        let named = |id: &ElementId, label: &str| {
            names.contains(&name_key(id.as_str())) || names.contains(&name_key(label))
        };
        if let Some(anchor) = self
            .thread
            .detours()
            .top()
            .and_then(|detour| detour.anchor.as_ref())
        {
            let label = self
                .board
                .layer(anchor.layer)
                .and_then(|layer| layer.element(&anchor.element))
                .map_or_else(
                    || anchor.element.as_str().to_owned(),
                    |element| element.label.clone(),
                );
            if named(&anchor.element, &label) {
                return Some(label);
            }
        }
        if self.thread.depth() >= DETOUR_DEPTH_MAX {
            return None;
        }
        let layer = self.thread.active_layer();
        let (id, label) = self
            .board
            .layer(layer)?
            .elements()
            .iter()
            .find_map(|element| {
                let mine = element.op.is_some()
                    && (element.state.is_shown() || element.state == ElementState::Planned);
                (mine && named(&element.id, &element.label))
                    .then(|| (element.id.clone(), element.label.clone()))
            })?;
        self.push_detail(&id, None, effects)?;
        Some(label)
    }

    /// The reply ended normally: what it left planned of the briefs' diagrams it was given is
    /// revealed next, after its other ops, as its last words play. Not when an op still queued
    /// would change the layer in front (a detour, a resume, a page): the parts stay planned and
    /// the summary names them.
    pub(in crate::engine) fn reveal_brief_leftovers(&mut self, stream: usize) {
        let staged = self.staging.take_briefs();
        let moves = self.queue.ops().any(|queued| {
            matches!(
                queued.op,
                NarrationOp::Detour { .. }
                    | NarrationOp::Detail { .. }
                    | NarrationOp::Resume
                    | NarrationOp::Page { .. }
            )
        });
        if moves {
            return;
        }
        for (layer, id) in staged {
            if layer != self.thread.active_layer() {
                continue;
            }
            let op = self.fresh_op();
            self.queue.push(QueuedOp::new(
                op,
                NarrationOp::Reveal {
                    target: id,
                    deep: true,
                },
                Anchor::Immediate { stream },
            ));
        }
    }

    /// An op id no stream uses: the core's own ops (a brief's staging, its leftover reveal).
    fn fresh_op(&mut self) -> OpId {
        let op = OpId(self.queue.next_op);
        self.queue.next_op = self.queue.next_op.saturating_add(1);
        op
    }
}
