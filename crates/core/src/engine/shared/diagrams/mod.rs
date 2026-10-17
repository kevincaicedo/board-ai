//! A research brief's diagram on the board (D-242). A brief's `## Diagram` is a `<diagram>`
//! block in the narration vocabulary (D-234). The core reads it with the narration's own parser
//! as the brief arrives, and stages it (D-235, D-236) when the brief reaches the talker.
//!
//! - **When.** With the request that carries the brief: a held `research` call's result (that
//!   reply's tool round), or the brief's report turn. A prefetch goes quietly with whatever is
//!   asked next, so its diagram waits until the talker uses the brief (a `research` call about
//!   it): a map nobody asked about never lands under another question. A diagram waiting for its
//!   reply's tool round belongs to that reply: when the reply stops first, a brief's goes back to
//!   waiting for use and a figure's is forgotten, so neither lands under the next question
//!   (D-270). No request goes out while the user draws (I14) or once the session ended, so no
//!   staging does either.
//! - **Where.** On the layer in front: the main board for an overview (a full page turns first,
//!   D-177); for a component brief, the detail layer anchored to that part, opened on it now
//!   when the board in front has it (D-238). Opening a layer or turning the page while the
//!   reply's earlier words still wait to draw would move their drawing there, so that staging
//!   waits until they have drawn (`stage`, D-270).
//! - **What the talker hears.** A note with the staged ids. Its title and labels are the source's
//!   words, which the talker reads only in the fenced text the diagram came with, so the note
//!   and the board summary name the parts by id (D-265). It reveals them as it explains (D-237),
//!   and what its reply leaves planned is revealed as the reply ends.
//! - **Untrusted.** The block is harness output (D-107): staging lays it out and draws it, nothing
//!   in it runs, and every tag or part that doesn't parse or fit is dropped and counted.
//! - **Provenance.** The staged diagram names who read the code, the source and the files read;
//!   the header shows it as a chip ("from Claude Code's brief · 23 files read").
//! - **A document's figures** (D-257) take the same path: when the talker reads a figure, its
//!   digest's `<diagram>` is staged with the next request and the talker told its ids; its Doc
//!   card says who read it, so the header shows no chip. A figure without an id of its own is
//!   `figure-<n>`, and one already staged on the layer in front, by its document and number, isn't
//!   staged again (D-269).

mod stage;

use crate::effect::{Dropped, Effect, LogRecord};
use crate::engine::Core;
use crate::engine::shared::BriefTask;
use crate::ids::{ElementId, LayerId, OpId};
use crate::limits::{BRIEF_DIAGRAMS_WAITING_MAX, TITLE_MAX_BYTES};
use crate::narration::diagram::{DiagramSpec, WrittenDiagram};
use crate::narration::tags::bounded;
use crate::narration::tags::diagram::DEFAULT_DIAGRAM_ID;
use crate::protocol::DiagramProvenance;
use crate::sources::brief::Brief;
use crate::sources::{BriefTopic, SourceId};

/// When a source's diagram goes on the board.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::engine) enum Carried {
    /// With this reply's next request: the tool round that carries a held call's result. The
    /// reply stopping first releases it (`release_reply_diagrams`).
    ThisReply,
    /// With the next request, whichever it is: it carries the brief's report.
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

/// A source's diagram, read, waiting for its brief (or its figure's read) to reach the talker.
#[derive(Clone, Debug)]
pub(in crate::engine) struct WaitingDiagram {
    source: SourceId,
    topic: BriefTopic,
    /// How BoardAI names it to the talker, in its own words and ids: "Claude Code's overview
    /// brief of inf-alloc", "Figure 2 of paper.pdf" (D-265).
    name: String,
    spec: DiagramSpec,
    /// Tags the narration's parser dropped from it.
    dropped: u32,
    /// Who read the code, for the header's chip; `None` for a document's figure, whose Doc card
    /// says who read it (D-257).
    provenance: Option<DiagramProvenance>,
    carried: Carried,
    /// The paths of the brief's citations the core read and checked: what its labels are held
    /// to (D-252). `None` for a document's figure, whose labels are the paper's words, not
    /// paths in a repo.
    read: Option<Vec<String>>,
    /// A document's figure: its number, so it is staged once per layer (D-269).
    figure: Option<u32>,
}

/// A staging that waits for the reply's earlier words to draw: it would open a layer or turn
/// the page under them (D-270).
#[derive(Clone, Debug)]
pub(in crate::engine) struct Deferred {
    waiting: WaitingDiagram,
    /// The op its staging takes, reserved when it was deferred so no stream's op reuses it.
    op: OpId,
    /// Every op before this one was queued before the staging: it waits for them.
    after: OpId,
    /// The part whose detail layer it opens; `None` stages on the layer in front.
    detail: Option<ElementId>,
}

impl WaitingDiagram {
    fn release(mut self) -> Option<Self> {
        if self.carried != Carried::ThisReply {
            return Some(self);
        }
        // A figure is read again the next time the talker reads it; a brief's waits for use.
        self.figure.is_none().then(|| {
            self.carried = Carried::OnUse;
            self
        })
    }
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
        let Some((mut spec, dropped)) = read_diagram(&diagram.text, effects) else {
            return BriefDiagram::Unreadable;
        };
        let source = brief_task.source;
        let entry = self.sources.shared.get(source);
        let title = entry
            .map(|entry| entry.info.title.clone())
            .unwrap_or_default();
        let harness = self.sources.shared.harness_name().to_owned();
        let name = match &brief_task.topic {
            BriefTopic::Overview => format!("{harness}'s overview brief of {title}"),
            BriefTopic::Component { name } => match entry.and_then(|entry| entry.component(name)) {
                Some((id, _)) => format!("{harness}'s brief on part {id} of {title}"),
                None => format!("{harness}'s brief on a part of {title}"),
            },
        };
        if spec.title == spec.id.as_str() {
            // No title of its own: the header names it after what it maps.
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
            name,
            spec,
            dropped,
            provenance: Some(DiagramProvenance {
                reader: harness,
                source: title,
                files_read: (!cached).then_some(brief_task.files_read),
            }),
            carried,
            read: Some(checked_paths(brief)),
            figure: None,
        };
        self.keep_waiting(waiting, effects);
        BriefDiagram::Staged(carried)
    }

    /// A shared document's figure the talker read (D-257): its digest's `<diagram>` waits to be
    /// staged with this reply's next request, like a held brief's, on the layer in front. `None`
    /// when that figure of that document is staged on that layer already (its ids were named
    /// then).
    pub(in crate::engine) fn wait_to_stage_figure(
        &mut self,
        source: SourceId,
        (number, caption): (u32, &str),
        file: &str,
        text: &str,
        effects: &mut Vec<Effect>,
    ) -> BriefDiagram {
        let Some((mut spec, dropped)) = read_diagram(text, effects) else {
            return BriefDiagram::Unreadable;
        };
        let label = format!("Figure {number}");
        if self.figure_on_board(source, number) {
            return BriefDiagram::None;
        }
        if self.figure_deferred(source, number) {
            return BriefDiagram::Staged(Carried::ThisReply);
        }
        if spec.title == spec.id.as_str() || spec.title.eq_ignore_ascii_case(&label) {
            // "Figure 1" alone says little in the header: its caption does.
            spec.title = bounded(&format!("{label} · {caption}"), TITLE_MAX_BYTES);
        }
        if spec.id.as_str() == DEFAULT_DIAGRAM_ID
            && let Some(id) = ElementId::parse(&format!("figure-{number}"))
        {
            // The digest's shape writes no id: every figure would be `diagram`, and a second
            // figure would read as staged already.
            spec.id = id;
        }
        let waiting = WaitingDiagram {
            source,
            topic: BriefTopic::component(&label),
            name: format!("{label} of {file}"),
            spec,
            dropped,
            provenance: None,
            carried: Carried::ThisReply,
            read: None,
            figure: Some(number),
        };
        self.keep_waiting(waiting, effects);
        BriefDiagram::Staged(Carried::ThisReply)
    }

    /// Whether figure `number` of `source` is staged on the layer in front and still there.
    fn figure_on_board(&self, source: SourceId, number: u32) -> bool {
        let layer = self.thread.active_layer();
        self.sources
            .figures
            .get(&(source, number, layer))
            .is_some_and(|id| {
                self.board
                    .staged(layer)
                    .iter()
                    .any(|staged| staged.id == *id)
            })
    }

    fn figure_deferred(&self, source: SourceId, number: u32) -> bool {
        self.sources.deferred.iter().any(|deferred| {
            deferred.waiting.source == source && deferred.waiting.figure == Some(number)
        })
    }

    /// Records a figure staged on `layer` as `id`; records whose diagram left the board go.
    fn figure_staged(&mut self, source: SourceId, number: u32, layer: LayerId, id: ElementId) {
        let board = &self.board;
        self.sources
            .figures
            .retain(|(_, _, layer), id| board.staged(*layer).iter().any(|staged| staged.id == *id));
        self.sources.figures.insert((source, number, layer), id);
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
        self.sources
            .deferred
            .retain(|deferred| !sources.contains(&deferred.waiting.source));
        self.sources
            .figures
            .retain(|(source, _, _), _| !sources.contains(source));
    }

    /// The talker uses a brief it already has (a `research` call about it): its diagram, if one
    /// waits for that, goes on the board with this reply's next request. True when one did.
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
                waiting.carried = Carried::ThisReply;
                true
            }
            None => false,
        }
    }

    /// The reply these diagrams waited for stopped, or ended without the request that carries
    /// them: a brief's goes back to waiting until the talker uses it, a figure's is forgotten
    /// (D-270). Neither lands under the next question.
    pub(in crate::engine) fn release_reply_diagrams(&mut self, effects: &mut Vec<Effect>) {
        let waiting = std::mem::take(&mut self.sources.diagrams);
        self.sources.diagrams = waiting
            .into_iter()
            .filter_map(WaitingDiagram::release)
            .collect();
        for deferred in std::mem::take(&mut self.sources.deferred) {
            let mut waiting = deferred.waiting;
            waiting.carried = Carried::ThisReply;
            if let Some(waiting) = waiting.release() {
                self.keep_waiting(waiting, effects);
            }
        }
    }
}

/// Reads a `<diagram>` block written outside the talker's stream; `None` when nothing in it
/// could be staged. Tags it dropped are counted.
fn read_diagram(text: &str, effects: &mut Vec<Effect>) -> Option<(DiagramSpec, u32)> {
    let read = WrittenDiagram::read(text);
    if read.dropped > 0 {
        effects.push(Effect::Log(LogRecord::Dropped {
            what: Dropped::DiagramPart,
            count: read.dropped,
        }));
    }
    read.spec.map(|spec| (spec, read.dropped))
}

/// The paths of a brief's citations, once checked: only what the core read is left (D-185).
fn checked_paths(brief: &Brief) -> Vec<String> {
    let mut paths: Vec<String> = brief
        .all_citations()
        .into_iter()
        .map(|citation| citation.path)
        .collect();
    paths.sort();
    paths.dedup();
    paths
}
