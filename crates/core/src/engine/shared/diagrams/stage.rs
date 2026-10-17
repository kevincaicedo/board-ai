//! Staging a source's diagram when the request that carries it goes out, or once the reply's
//! earlier words have drawn (D-270), and what the talker is told about it: ids only (D-265).

use std::fmt::Write as _;

use super::{Carried, Deferred, WaitingDiagram};
use crate::board::staged::StageRefused;
use crate::effect::Effect;
use crate::engine::Core;
use crate::engine::repo_names::{RepoNames, marked_unread, parts_were};
use crate::engine::reveal::{StageFor, StagedHere, not_staged_because};
use crate::ids::{ElementId, LayerId, OpId};
use crate::limits::DETOUR_DEPTH_MAX;
use crate::narration::diagram::{DiagramSpec, SpecPart};
use crate::protocol::StagedPart;
use crate::sources::{BriefTopic, name_key};

/// Parts named in the note to the talker; the rest are counted.
const NOTE_PARTS_MAX: usize = 24;

/// Where a component brief's diagram goes.
enum Place {
    /// The layer in front.
    Here,
    /// The detail layer in front, anchored to the part.
    InFront(ElementId),
    /// A detail layer to open on the part, which the layer in front has.
    Open(ElementId),
}

impl Core {
    /// A talker request is going out: the source diagrams it carries are staged first, so the
    /// board summary and a note name their ids. A new turn starts with no brief diagram of its
    /// own: what an earlier reply left planned stays so, the summary says what is planned, and
    /// what waited for an earlier reply's tool round is released.
    pub(in crate::engine) fn stage_brief_diagrams(
        &mut self,
        new_turn: bool,
        effects: &mut Vec<Effect>,
    ) {
        if new_turn {
            let _ = self.staging.take_briefs();
            self.release_reply_diagrams(effects);
        }
        let mut due = Vec::new();
        self.sources.diagrams.retain(|waiting| {
            let now = waiting.carried != Carried::OnUse;
            if now {
                due.push(waiting.clone());
            }
            !now
        });
        // Ops still queued are the reply's earlier words: a layer that opens or a page that
        // turns under them would take their drawing with it.
        let earlier = !self.queue.is_empty();
        for waiting in due {
            self.stage_or_defer(waiting, earlier, effects);
        }
    }

    fn stage_or_defer(
        &mut self,
        waiting: WaitingDiagram,
        earlier: bool,
        effects: &mut Vec<Effect>,
    ) {
        let place = self.place_for(&waiting);
        if earlier && let Place::Open(part) = &place {
            let part = part.clone();
            self.defer(waiting, Some(part));
            return;
        }
        let anchor = match place {
            Place::Here => None,
            Place::InFront(part) => Some(part),
            Place::Open(part) => self.push_detail(&part, None, effects).map(|_| part),
        };
        let op = self.fresh_op();
        match self.stage_waiting(op, &waiting, !earlier, effects) {
            Ok(here) => {
                let note = self.staged_note(&waiting, &here, anchor.as_ref());
                self.context.notice(note);
            }
            Err(StageRefused::NeedsPage) if earlier => self.defer(waiting, None),
            Err(refused) => self.not_staged(&waiting, &refused),
        }
    }

    /// Lays the diagram out on the layer in front, held to what its brief's checked citations
    /// name (D-252), and records it for the reply's leftovers and, for a figure, once per layer.
    fn stage_waiting(
        &mut self,
        op: OpId,
        waiting: &WaitingDiagram,
        may_turn_page: bool,
        effects: &mut Vec<Effect>,
    ) -> Result<StagedHere, StageRefused> {
        let spec = checked(waiting);
        let stage_for = StageFor::Source {
            name: waiting.name.clone(),
            provenance: waiting.provenance.clone(),
        };
        let here = self.stage_here(op, &spec, stage_for, may_turn_page, effects)?;
        self.staging
            .brief_staged(here.layer, here.staged.id.clone());
        if let Some(number) = waiting.figure {
            self.figure_staged(waiting.source, number, here.layer, here.staged.id.clone());
        }
        Ok(here)
    }

    /// Holds the staging until every op queued before it has drawn; its op is reserved now.
    fn defer(&mut self, waiting: WaitingDiagram, detail: Option<ElementId>) {
        let op = self.fresh_op();
        let note = deferred_note(&waiting, detail.as_ref());
        self.context.notice(note);
        self.sources.deferred.push_back(Deferred {
            waiting,
            op,
            after: op,
            detail,
        });
    }

    /// Stages what waited for the reply's earlier words, once the next op in line came after it
    /// (every earlier op drew, or will never draw). Called before each op runs.
    pub(in crate::engine) fn stage_deferred(&mut self, effects: &mut Vec<Effect>) {
        // Each pass stages one or stops; the deferred queue's length bounds the loop.
        for _ in 0..self.sources.deferred.len() {
            let Some(front) = self.sources.deferred.front() else {
                return;
            };
            if self.queue.ops().any(|queued| queued.id < front.after) {
                return;
            }
            let Some(deferred) = self.sources.deferred.pop_front() else {
                return;
            };
            if self.has_ended() {
                continue;
            }
            if let Some(part) = &deferred.detail {
                let _ = self.push_detail(part, None, effects);
            }
            match self.stage_waiting(deferred.op, &deferred.waiting, true, effects) {
                Ok(here) if here.staged.dropped > 0 => self.context.notice(format!(
                    "The diagram of {} is staged now; {} of its parts were dropped (a repeated or \
                     taken id, or a connector naming nothing).",
                    deferred.waiting.name, here.staged.dropped
                )),
                Ok(_) => {}
                Err(refused) => self.not_staged(&deferred.waiting, &refused),
            }
        }
    }

    fn not_staged(&mut self, waiting: &WaitingDiagram, refused: &StageRefused) {
        self.context.notice(format!(
            "The diagram of {} was not staged: {}. Draw what you explain with your own tags, a \
             part at a time.",
            waiting.name,
            not_staged_because(refused)
        ));
    }

    /// What the talker is told about a source's staged diagram: where it is, its ids, and how
    /// to reveal them. Never its title or labels: they are the source's words (D-265).
    fn staged_note(
        &self,
        waiting: &WaitingDiagram,
        here: &StagedHere,
        anchor: Option<&ElementId>,
    ) -> String {
        let staged = &here.staged;
        let place = match anchor {
            Some(part) => format!(
                "on the detail layer anchored to {}, open in front of the board",
                part.as_str()
            ),
            None => "on the board".to_owned(),
        };
        let mut note = format!(
            "The diagram of {} is staged {place} (id {}); nothing of it is drawn yet.",
            waiting.name,
            staged.id.as_str()
        );
        if here.page_turned {
            note.push_str(" The page turned to make room for it.");
        }
        let parts = self.part_ids(here.layer, &staged.id);
        reveal_lines(
            &mut note,
            &parts,
            waiting.dropped.saturating_add(staged.dropped),
        );
        let unread = unread_ids(waiting);
        if !unread.is_empty() {
            let _ = write!(
                note,
                " {} marked \"?\": the brief's checked citations name none of their paths ({}).",
                parts_were(unread.len()),
                unread.join(", ")
            );
        }
        note
    }

    /// The ids of diagram `id`'s parts on `layer` (not its connectors or notes).
    fn part_ids(&self, layer: LayerId, id: &ElementId) -> Vec<String> {
        self.board
            .staged(layer)
            .iter()
            .find(|diagram| diagram.id == *id)
            .map(|diagram| {
                diagram
                    .entries
                    .iter()
                    .filter(|entry| {
                        entry.ends.is_none() && !matches!(entry.draw, StagedPart::Note { .. })
                    })
                    .map(|entry| entry.id.as_str().to_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Where a diagram goes: a component brief's on its part's detail layer, in front already
    /// or to be opened (D-238, within the detour depth); anything else on the layer in front.
    fn place_for(&self, waiting: &WaitingDiagram) -> Place {
        let BriefTopic::Component { name } = &waiting.topic else {
            return Place::Here;
        };
        if waiting.figure.is_some() {
            return Place::Here;
        }
        let mut names = vec![name_key(name)];
        if let Some((id, label)) = self
            .sources
            .shared
            .get(waiting.source)
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
                .map(|element| element.label.as_str())
                .unwrap_or_default();
            if named(&anchor.element, label) {
                return Place::InFront(anchor.element.clone());
            }
        }
        if self.thread.depth() >= DETOUR_DEPTH_MAX {
            return Place::Here;
        }
        let layer = self.thread.active_layer();
        self.board
            .layer(layer)
            .and_then(|board| {
                board.elements().iter().find(|element| {
                    let mine = element.op.is_some()
                        && (element.state.is_shown()
                            || element.state == crate::board::ElementState::Planned);
                    mine && named(&element.id, &element.label)
                })
            })
            .map_or(Place::Here, |element| Place::Open(element.id.clone()))
    }

    /// An op id no stream uses: the core's own ops (a brief's staging, a leftover reveal).
    pub(in crate::engine) fn fresh_op(&mut self) -> OpId {
        let op = OpId(self.queue.next_op);
        self.queue.next_op = self.queue.next_op.saturating_add(1);
        op
    }
}

/// The diagram held to its brief's checked citations (D-252); a figure's labels are the
/// paper's words, not paths.
fn checked(waiting: &WaitingDiagram) -> DiagramSpec {
    match &waiting.read {
        Some(paths) => {
            let read = RepoNames::of_paths(paths.iter().map(String::as_str));
            marked_unread(&waiting.spec, |label| read.verdict(label, false)).0
        }
        None => waiting.spec.clone(),
    }
}

/// The ids of the parts whose labels name a path the brief's checked citations don't.
fn unread_ids(waiting: &WaitingDiagram) -> Vec<String> {
    let Some(paths) = &waiting.read else {
        return Vec::new();
    };
    let read = RepoNames::of_paths(paths.iter().map(String::as_str));
    let (checked, _) = marked_unread(&waiting.spec, |label| read.verdict(label, false));
    waiting
        .spec
        .items
        .iter()
        .zip(&checked.items)
        .filter(|(before, after)| before != after)
        .filter_map(|(before, _)| before.part.id().map(|id| id.as_str().to_owned()))
        .collect()
}

/// What the talker hears when a diagram's staging waits for the reply's earlier words.
fn deferred_note(waiting: &WaitingDiagram, detail: Option<&ElementId>) -> String {
    let place = match detail {
        Some(part) => format!(
            "on a detail layer anchored to {}, opened then",
            part.as_str()
        ),
        None => "on a fresh page".to_owned(),
    };
    let mut note = format!(
        "The diagram of {} will be staged {place} once the words you already wrote have played \
         (id {}); nothing of it is drawn yet.",
        waiting.name,
        waiting.spec.id.as_str()
    );
    let parts: Vec<String> = waiting
        .spec
        .items
        .iter()
        .filter(|item| !matches!(item.part, SpecPart::Edge { .. } | SpecPart::Note { .. }))
        .filter_map(|item| item.part.id().map(|id| id.as_str().to_owned()))
        .collect();
    reveal_lines(&mut note, &parts, waiting.dropped);
    note
}

/// The parts by id, at most [`NOTE_PARTS_MAX`] then how many more, and how to reveal them.
fn reveal_lines(note: &mut String, parts: &[String], dropped: u32) {
    let more = parts.len().saturating_sub(NOTE_PARTS_MAX);
    let mut named: Vec<&str> = parts
        .iter()
        .take(NOTE_PARTS_MAX)
        .map(String::as_str)
        .collect();
    let more_text = format!("and {more} more");
    if more > 0 {
        named.push(&more_text);
    }
    let _ = write!(
        note,
        " Its parts, by id (their words are in the fenced text it came with): {}.",
        named.join(", ")
    );
    note.push_str(
        " Reveal each with <reveal id=\"…\"/> right before the words that explain it \
         (deep=\"true\" on a container brings what it holds), and don't write it again as a \
         <diagram>. Whatever you haven't revealed when your reply ends is drawn then.",
    );
    if dropped > 0 {
        let _ = write!(
            note,
            " {dropped} of its tags or parts were dropped (malformed, misplaced, a repeated or \
             taken id, or a connector naming nothing)."
        );
    }
}
