//! The wire between the core and the UI (LLD §3.7). These types are exported to TypeScript with
//! ts-rs into `ui/src/protocol/generated/`; `just check` fails when the export is stale.
//!
//! Events go core → UI in batches stamped with session time, so the UI's animation clock follows
//! the schedule the core computed. Commands go UI → runtime; the runtime forwards the ones that
//! concern the conversation to the core as `Input::Command`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::geometry::{Point, Rect};
use crate::ids::{ElementId, LayerId, OpId, SurfaceId};
use crate::time::SessionTime;

/// Every UI event produced while handling one step, in order, stamped with the step's time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UiBatch {
    pub at: SessionTime,
    pub events: Vec<UiEvent>,
}

/// One line of a session's `events.jsonl` that the UI can replay: `{"v":1,"t":…,"k":"ui",…}`.
/// Lines of other kinds (`"k":"input"`) are for re-simulation and the UI skips them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UiLogLine {
    pub v: u8,
    pub t: SessionTime,
    pub k: UiLogKind,
    pub event: UiEvent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum UiLogKind {
    Ui,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type")]
#[ts(export)]
pub enum UiEvent {
    /// The orb's state; `level_percent` is the live voice energy, 0 to 100.
    Orb {
        state: OrbState,
        level_percent: u8,
    },
    /// A small pulse: a backchannel was heard and BoardAI keeps going.
    Acknowledge,
    /// A live caption. Captions show text as written; speech gets the speakable form.
    Caption {
        who: Speaker,
        text: String,
        is_final: bool,
    },
    /// The explanation plan, the current step and the open detours, outermost first.
    Thread {
        steps: Vec<String>,
        current: Option<u8>,
        detours: Vec<String>,
    },
    /// Start drawing an op; finish it within `budget_ms` so drawing never piles up (LLD §2.4).
    OpStart {
        layer: LayerId,
        op: DrawOp,
        budget_ms: u32,
    },
    /// Stop the stroke in progress on this layer where it is (LLD §2.7).
    Freeze {
        layer: LayerId,
    },
    /// Continue the frozen stroke on this layer from where it stopped.
    Resume {
        layer: LayerId,
        budget_ms: u32,
    },
    /// A detour opened a new layer; the board under it softens into the background.
    LayerPush {
        layer: LayerId,
        title: String,
    },
    /// The detour closed; its layer goes away and the one under it returns to focus.
    LayerPop {
        layer: LayerId,
    },
    /// Frame this region of the layer (camera follow) and sharpen the focused element.
    Camera {
        layer: LayerId,
        frame: Rect,
        focus: Option<ElementId>,
    },
    SurfaceShow {
        surface: Surface,
    },
    SurfaceUpdate {
        id: SurfaceId,
        update: SurfaceUpdate,
    },
    SurfaceDismiss {
        id: SurfaceId,
    },
    /// A captured item (note, task, memory) flying to the brain edge, or a source chip.
    Chip {
        kind: ChipKind,
        text: String,
    },
    /// A failure the user hears about: the spoken line and the detail for the caption.
    Problem {
        spoken: String,
        detail: String,
    },
}

/// The orb states on the design canvas (frame 10a).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum OrbState {
    Idle,
    Listening,
    Thinking,
    Speaking,
    Drawing,
    Muted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum Speaker {
    User,
    Assistant,
}

/// One drawing step with final geometry. The core lays the board out; the UI only draws.
/// Stroke randomness in the UI is seeded by the element id, so a replay draws the same strokes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind")]
#[ts(export)]
pub enum DrawOp {
    /// Outline, then the label written letter by letter, then the hachure fill. `stack` above 1
    /// draws that many offset copies behind it ("three API pods").
    Node {
        id: ElementId,
        op: OpId,
        shape: NodeShape,
        label: String,
        rect: Rect,
        stack: u8,
    },
    /// A connector from source to target, then its arrowhead, then its label.
    Edge {
        id: ElementId,
        op: OpId,
        from: ElementId,
        to: ElementId,
        path: EdgePath,
        label: Option<String>,
        style: LineStyle,
    },
    /// An orange pen mark around or under the target.
    Annotate {
        id: ElementId,
        op: OpId,
        target: ElementId,
        mark: Mark,
        rect: Rect,
        text: Option<String>,
    },
    /// Strike through the target's label and write the new one beside it.
    Strike {
        op: OpId,
        target: ElementId,
        label: String,
        rect: Rect,
    },
    /// A dot travels along these points, in step with the code highlight.
    Trace {
        id: ElementId,
        op: OpId,
        points: Vec<Point>,
    },
}

/// Straight when `control` is `None`, otherwise one quadratic curve.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EdgePath {
    pub start: Point,
    pub control: Option<Point>,
    pub end: Point,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum NodeShape {
    /// A box.
    Service,
    /// A cylinder.
    Db,
    /// A box with message dividers.
    Queue,
    /// A person or client.
    Actor,
    /// A dashed boundary around its children.
    Group,
    /// Handwritten text with no outline.
    Note,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum LineStyle {
    Solid,
    Dashed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum Mark {
    Circle,
    Underline,
    Arrow,
    Bracket,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind")]
#[ts(export)]
pub enum Surface {
    /// A snippet from a file read in this session; `first_line` is 1-based.
    Code {
        id: SurfaceId,
        path: String,
        first_line: u32,
        lines: Vec<String>,
        highlight: Option<u32>,
    },
    Compare {
        id: SurfaceId,
        options: Vec<String>,
        criteria: Vec<String>,
    },
    Recap {
        id: SurfaceId,
        bullets: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind")]
#[ts(export)]
pub enum SurfaceUpdate {
    /// Move the code card's highlight to this 1-based line.
    Highlight { line: u32 },
    /// Fill one compare cell; rows are options and columns are criteria, from 0.
    Cell { row: u8, col: u8, mark: CellMark },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind")]
#[ts(export)]
pub enum CellMark {
    Yes,
    No,
    Depends,
    Text { text: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ChipKind {
    Note,
    Task,
    Memory,
    Source,
}

/// Commands from the UI. Pointing and the mic go to the core; the rest are runtime views.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type")]
#[ts(export)]
pub enum UiCommand {
    /// What the pointer is over, or `None` when it left every target.
    Pointer {
        target: Option<PointerTarget>,
    },
    Mic {
        action: MicAction,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind")]
#[ts(export)]
pub enum PointerTarget {
    Element { layer: LayerId, id: ElementId },
    CodeLine { surface: SurfaceId, line: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum MicAction {
    Mute,
    Unmute,
    /// Push-to-talk key pressed.
    TalkDown,
    /// Push-to-talk key released.
    TalkUp,
}

#[cfg(test)]
mod tests {
    use super::{DrawOp, NodeShape, UiBatch, UiEvent};
    use crate::geometry::Rect;
    use crate::ids::{ElementId, LayerId, OpId};
    use crate::time::SessionTime;

    #[test]
    fn events_serialize_as_tagged_objects() {
        let id = ElementId::parse("lb").expect("valid id");
        let batch = UiBatch {
            at: SessionTime::from_ms(1200),
            events: vec![UiEvent::OpStart {
                layer: LayerId::MAIN,
                op: DrawOp::Node {
                    id,
                    op: OpId(3),
                    shape: NodeShape::Service,
                    label: "Load balancer".to_owned(),
                    rect: Rect {
                        x: 0,
                        y: 0,
                        w: 160,
                        h: 64,
                    },
                    stack: 1,
                },
                budget_ms: 900,
            }],
        };
        let json = serde_json::to_string(&batch).expect("serializes");
        assert!(json.starts_with(
            r#"{"at":1200,"events":[{"type":"OpStart","layer":0,"op":{"kind":"Node","id":"lb""#
        ));
        let back: UiBatch = serde_json::from_str(&json).expect("round trips");
        assert_eq!(back, batch);
    }
}
