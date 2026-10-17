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
pub use crate::sources::DiagramProvenance;
use crate::sources::tray::{DropOverlay, SourceAction, SourcesTray};
use crate::time::SessionTime;
use crate::views::View;

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
    /// `question` is the user's question that opened it, for the card's subtitle ("Detour · why
    /// Redis first?"); `None` when the talker opened it on its own. `anchor` makes it a detail
    /// layer (drill-down, D-188, artboard 15b): the camera moves into that element's box and the
    /// layer fills the board's area, with the breadcrumbs of the layers it drills through.
    /// `None` for a detour, drawn in its card beside the board (frame 4).
    LayerPush {
        layer: LayerId,
        title: String,
        question: Option<String>,
        anchor: Option<DetailAnchor>,
    },
    /// The detour closed; its layer goes away and the one under it returns to focus.
    LayerPop {
        layer: LayerId,
    },
    /// Frame this region of the layer, in world units, and sharpen the focused element. The core
    /// sends a layer's first frame with its first drawing and follows as the board grows (its
    /// bounds plus a margin); a `focus` tag frames one element and holds the camera until the
    /// board grows past that frame. The UI fits the frame into the stage area clear of the
    /// thread bar and the captions, never enlarging past natural size.
    ///
    /// `enlarge`: the frame closes up on part of a staged diagram, drawn in its small sizes
    /// (canvas 15), so the UI may enlarge it past natural size, up to
    /// `--diagram-camera-scale-max`; a dense part then reads like a sketch. Every other frame is
    /// never enlarged. Logs written before it existed read it as false.
    Camera {
        layer: LayerId,
        frame: Rect,
        focus: Option<ElementId>,
        #[serde(default)]
        enlarge: bool,
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
    /// `source` is the code location a source chip opens when clicked (a `file:line` the talker
    /// cited from a file read this session); `None` for other chips and for a file a coding
    /// harness touched.
    Chip {
        kind: ChipKind,
        text: String,
        source: Option<SourceRef>,
    },
    /// A failure the user hears about: the spoken line and the detail for the caption.
    Problem {
        spoken: String,
        detail: String,
    },
    /// The idle suggestion under the orb (frame 1: "Pick up Raft log compaction where we left
    /// off?"). Sent when the session starts with one, and cleared with `None` the first time
    /// the user speaks.
    Suggestion {
        text: Option<String>,
    },
    /// A new session began (the app started, or the first words after an idle timeout ended
    /// the last one). Always the first event of a session: the UI starts from an empty stage,
    /// and session time starts again from zero. `session` is the store's id for it; `None`
    /// when memory is off.
    SessionStart {
        session: Option<String>,
    },
    /// Show a view summoned by voice (LLD §3.7). One view is open at a time; a new one
    /// replaces it. `today` (`YYYY-MM-DD`, UTC) is the day it was read, so "today" labels
    /// replay the same later.
    ViewShow {
        view: View,
        today: String,
    },
    /// The open view goes away: by voice, by Esc, or because the board draws again.
    ViewDismiss,
    /// "Open settings": the UI shows the settings screen (the first-run screen, LLD §3.8)
    /// over the stage. A replay ignores it.
    OpenSettings,
    /// The session ended (D-145): the stage clears to the idle stage until the next session's
    /// `SessionStart`. The core sends it when the user ended the session by voice and its last
    /// line has played; the orchestrator, which owns the session's life (LLD §3.6), ends the
    /// session when it applies it, and sends it itself after the idle timeout. It is never
    /// written to the session's log, so a replay never sees it.
    SessionEnd,
    /// You draw (artboard 13): you started (`drawing`) or stopped drawing. While it is on, the
    /// drawing tools show, the top reads "You're drawing · BoardAI is watching", and BoardAI
    /// waits: no talker request starts until you finish.
    DrawMode {
        drawing: bool,
    },
    /// One of your shapes as the core keeps it: the committed copy of what the UI previewed
    /// while the pointer was down. It is re-sent whole when it changes (a pen stroke joins it, a
    /// spoken label lands), and the UI replaces its drawing, keyed by the shape's id.
    UserShape {
        layer: LayerId,
        shape: UserShape,
    },
    /// Your shape `id` was undone.
    UserShapeRemove {
        layer: LayerId,
        id: ElementId,
    },
    /// While you speak in drawing mode: the words so far will label `target` ("…and this one
    /// is the cache" → will label this shape). `None` clears the hint.
    LabelHint {
        hint: Option<LabelHint>,
    },
    /// The board turned to a fresh, empty page (D-177): the main layer's drawing leaves with a
    /// page turn and layer 0 starts again with nothing on it and no camera frame, so its next
    /// stroke brings a new `Camera`. Nothing drawn moved (I6): the old page, with the shapes you
    /// drew on it, is kept whole in the session's record and export, and leaves the screen with
    /// them. Open detours were closed first (`LayerPop`s before it).
    /// `page` is the new page's number, from 2; `title` is the talker's name for it, `None`
    /// when the user asked for a clean board.
    PageTurn {
        page: u16,
        title: Option<String>,
    },
    /// A staged diagram on `layer` (D-187, canvas 15): its title for the stage's header, and the
    /// top-level part the narration is revealing, from 1, of `parts` ("part 3 of 4"); `part` is
    /// 0 when it is laid out and nothing of it is drawn yet. Sent when it is staged and when the
    /// narration moves to another part. A detail layer's diagram names its own parts; its title
    /// in the breadcrumbs is the layer's (`LayerPush.title`).
    ///
    /// `provenance`: a research brief's diagram names its reader (D-242), for the chip above it;
    /// `None` for the talker's own, and in logs written before it.
    Diagram {
        layer: LayerId,
        title: String,
        part: u8,
        parts: u8,
        provenance: Option<DiagramProvenance>,
    },
    /// The Sources tray (artboard 14), whole, each time it changes: this session's sources, the
    /// project's, their states, and the files the harness is reading (D-184).
    Sources {
        tray: SourcesTray,
    },
    /// The drop overlay (14b) while something is dragged over the window; `None` once it left
    /// or dropped.
    SourceDrop {
        drop: Option<DropOverlay>,
    },
}

/// Where a detail layer drills in (D-188): an element of the layer under it and that element's
/// box there, in world units, which the camera moves into as the detail layer opens; its
/// category tones the detail card's edge (artboard 15b), and none gives the plain edge.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DetailAnchor {
    pub layer: LayerId,
    pub element: ElementId,
    pub rect: Rect,
    pub category: Option<Category>,
}

/// A line of a staged diagram's handwriting where the core placed it: `at` is on the line's centre
/// (the UI writes with `dominant-baseline: central`), at its left end or its middle as the part
/// says.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TextAt {
    pub text: String,
    pub at: Point,
}

/// A staged diagram's categories (canvas 15): each pairs a stroke for frames and swatches with a
/// soft fill for boxes, in the order a diagram first uses them. The legend names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum Category {
    Slate,
    Violet,
    Blue,
    Green,
}

/// One part of a staged diagram (D-187), laid out with the whole diagram before its first stroke
/// and drawn when the narration reveals it (`DrawOp::Reveal`). Its sizes are the canvas's small
/// diagram sizes (canvas 15), not a sketch's. Each part is an element of its own: it can be
/// pointed at, marked, joined and struck by its id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "part")]
#[ts(export)]
pub enum StagedPart {
    /// A container: its frame in its category's stroke (the light grey without one), dashed
    /// when `dashed`, then its label (`--diagram-frame-label`) inside the top-left corner, and
    /// its note in the second ink (`--diagram-note`), after the label or on the line under it.
    /// Both are written from their `at`, left-aligned.
    Frame {
        id: ElementId,
        rect: Rect,
        label: TextAt,
        note: Option<TextAt>,
        category: Option<Category>,
        dashed: bool,
    },
    /// A box, pill, cylinder, queue, actor or circle: its outline (dashed when `dashed`), then
    /// its label (`--diagram-label`), prefixed by its step number ("1. round up to a class")
    /// when it is a numbered step, then the note under the label in the second ink, then its
    /// category's soft fill under the ink.
    Node {
        id: ElementId,
        shape: NodeShape,
        rect: Rect,
        label: String,
        note: Option<String>,
        step: Option<u8>,
        category: Option<Category>,
        dashed: bool,
    },
    /// A remark written inside the diagram in the second ink (`--diagram-note`), one entry per
    /// line, left-aligned from the top of `rect`.
    Note {
        id: ElementId,
        rect: Rect,
        lines: Vec<String>,
    },
    /// A struct with its fields in a row: each field's box with its name in the code font
    /// (`--diagram-field`), then the struct's name under them (`--diagram-label`), left-aligned.
    /// Each field is its own element (`arena.2`), as a cell is.
    Record {
        id: ElementId,
        label: String,
        fields: Vec<CellBox>,
    },
    /// A row (or column) of cells, as `DrawOp::Cells`, in the diagram's sizes: values in the code
    /// font (`--diagram-field`), the name and index labels in the second ink.
    Cells {
        id: ElementId,
        label: String,
        cells: Vec<CellBox>,
        vertical: bool,
    },
    /// A connector, its arrowhead, then its label (`--diagram-edge`) centred on its `at`, as
    /// `DrawOp::Edge`.
    Edge {
        id: ElementId,
        from: ElementId,
        to: ElementId,
        path: EdgePath,
        label: Option<TextAt>,
        style: LineStyle,
    },
    /// A legend entry, drawn with the first part of its category: a small dashed swatch in the
    /// category's stroke at the left of `rect`, then the category's name.
    Legend {
        id: ElementId,
        rect: Rect,
        category: Category,
        label: String,
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
    /// Outline, then the label written letter by letter, then the hachure fill in `fill`'s tone
    /// (no fill when `None`). `stack` above 1 draws that many offset copies behind it ("three
    /// API pods"). A group is re-sent with the same id and op when it grows (D-035). `link` is
    /// the line from a tree node's parent (its bottom centre to this node's top centre), drawn
    /// before the node.
    Node {
        id: ElementId,
        op: OpId,
        shape: NodeShape,
        label: String,
        rect: Rect,
        stack: u8,
        fill: Option<Fill>,
        link: Option<EdgePath>,
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
    /// An orange pen mark around or under the target; a `Highlight` hachures `rect` in `fill`
    /// instead (the core always names the tone of a highlight, and none for a pen mark).
    Annotate {
        id: ElementId,
        op: OpId,
        target: ElementId,
        mark: Mark,
        rect: Rect,
        text: Option<String>,
        fill: Option<Fill>,
    },
    /// Strike through the target's label and write the new one above it (D-032).
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
    /// A sequence diagram participant: its head box, the label in it, then its dashed lifeline
    /// down from the box's bottom centre to `lifeline_bottom`. Lifelines grow with the messages
    /// (`Message.extend`); the head never moves.
    Participant {
        id: ElementId,
        op: OpId,
        label: String,
        rect: Rect,
        lifeline_bottom: i32,
    },
    /// A message between two lifelines, in its own row below the previous one: first the
    /// lifeline stretches it needs (`extend`, quick dashed strokes), then the arrow along
    /// `points` (two points, or four for a message to itself), its head, then the label above
    /// it. A `Dashed` style is a return.
    Message {
        id: ElementId,
        op: OpId,
        from: ElementId,
        to: ElementId,
        points: Vec<Point>,
        label: Option<String>,
        style: LineStyle,
        extend: Vec<Lifeline>,
    },
    /// A row (or a column, `vertical`) of cells: an array, a memory page, hash buckets. Its
    /// label (left of a row, above a column), the outline and dividers, each value, then the
    /// index labels (under a row, left of a column). Each cell is its own element (`arr.3`), so
    /// it can be pointed at, marked, written and struck.
    Cells {
        id: ElementId,
        op: OpId,
        label: String,
        cells: Vec<CellBox>,
        vertical: bool,
    },
    /// Write a label into an element that has none (an empty cell), inside `rect`. A target
    /// that already has a label gets a `Strike` instead.
    Write {
        op: OpId,
        target: ElementId,
        label: String,
        rect: Rect,
    },
    /// Parts of a staged diagram (D-187) the narration just named, drawn one after another in a
    /// hand's order within one budget: a frame and its label before what is inside it, both
    /// ends before what joins them, a legend entry with the first part of its category. They
    /// were laid out with the whole diagram before its first stroke, so nothing drawn moves
    /// (I6); they freeze and resume as one stroke. `id` names the reveal, not an element.
    Reveal {
        id: ElementId,
        op: OpId,
        parts: Vec<StagedPart>,
    },
}

/// A stretch of one participant's lifeline, from `top` down to `bottom` at `x`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Lifeline {
    pub participant: ElementId,
    pub x: i32,
    pub top: i32,
    pub bottom: i32,
}

/// One cell of a `Cells` op: its own element id, its box, its value (empty for an empty cell)
/// and its index label, if shown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CellBox {
    pub id: ElementId,
    pub rect: Rect,
    pub value: String,
    pub index: Option<String>,
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
    /// A light, solid frame around its children with its label written above it (D-032).
    Group,
    /// Handwritten text with no outline.
    Note,
    /// A circle (an ellipse for a long label): tree nodes.
    Circle,
    /// A box with fully rounded ends: a state of a state machine (canvas 15's `Free`,
    /// `Leased(kind)`).
    Pill,
}

/// The hachure tones under a node's ink (the canvas's sketch fills). The core picks one per
/// subject, deterministically, unless the talker names one (`fill="mint"`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum Fill {
    Lilac,
    Sky,
    Mint,
    Blush,
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
    /// A hachure over the target in a tone (`Annotate.fill`), under the ink.
    Highlight,
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
    /// Options are the columns and criteria are the rows, as on the canvas; `title` heads the
    /// card ("Raft vs Paxos vs ZAB"), and the UI joins the options with "vs" without one.
    Compare {
        id: SurfaceId,
        options: Vec<String>,
        criteria: Vec<String>,
        title: Option<String>,
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
    /// Fill one compare cell: `row` indexes the criteria and `col` the options, from 0.
    Cell { row: u8, col: u8, mark: CellMark },
    /// An orange pen mark on part of a code line (frame 5's underline under `s.cache.Get(`):
    /// `line` is 1-based, `columns` are the characters it covers.
    Mark {
        line: u32,
        columns: ColumnRange,
        mark: Mark,
    },
}

/// Characters of a line, counted in Unicode scalar values from 0, half-open `[begin, end)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ColumnRange {
    pub begin: u32,
    pub end: u32,
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

/// A line of a file read this session (law 6): what a source chip opens. `path` is the path the
/// file was read with; `line` is 1-based.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SourceRef {
    pub path: String,
    pub line: u32,
}

/// Commands from the UI. Pointing, the mic and closing a view go to the core.
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
    /// Esc (PRODUCT §8.9): BoardAI stops at once, like a confirmed interruption with no words.
    /// The stroke in flight freezes, what was heard is committed, and the floor goes idle.
    Stop,
    /// Esc: close the open view.
    CloseView,
    /// A source chip was clicked: open the code card on that line. The core shows it only for a
    /// file read this session, and ignores anything else.
    OpenSource {
        path: String,
        line: u32,
    },
    /// You draw (artboard 13): the pen button or `D`, a finished shape, undo, Done (or Enter),
    /// and Esc.
    Draw {
        action: DrawAction,
    },
    /// The Sources tray's controls (artboard 14): open or close it, keep, stop keeping, remove.
    /// No path ever comes this way: a source is added by a native drop or the chooser (D-173).
    Sources {
        action: SourceAction,
    },
}

/// What you do with the drawing tools.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind")]
#[ts(export)]
pub enum DrawAction {
    /// The pen button or `D`: BoardAI stops if it was speaking (an interruption) and watches
    /// while you draw.
    Start,
    /// A shape you finished (the pointer came up), in world units of `layer`.
    Shape { layer: LayerId, drawn: DrawnShape },
    /// Undo your last shape of this drawing (a pen shape loses its last stroke).
    Undo,
    /// Done or Enter: BoardAI looks at your drawing and answers.
    Done,
    /// Esc: you stop drawing without asking BoardAI anything; your shapes stay.
    Cancel,
}

/// One finished shape as the UI captured it. The core parses it once (bounded point counts,
/// coordinates in range, nothing smaller than a click) before it reaches the board.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "tool")]
#[ts(export)]
pub enum DrawnShape {
    /// The pen: one stroke's sampled points, at most `USER_STROKE_POINTS_MAX`.
    Pen { points: Vec<Point> },
    /// The box tool: the rectangle you dragged out.
    Box { rect: Rect },
    /// The arrow tool: where your drag started and ended.
    Arrow { start: Point, end: Point },
}

/// A shape you drew (artboard 13), drawn in your ink. Its id (`u1`, `u2`…) is the session's, so
/// BoardAI can name it (`annotate target="u2"`), but BoardAI never moves or redraws it (I6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UserShape {
    pub id: ElementId,
    pub kind: UserShapeKind,
    /// What you said it is ("cache"): written inside a box or a pen shape, above an arrow.
    pub label: Option<String>,
    /// A second thing you said about it ("source of truth"), written under it.
    pub note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind")]
#[ts(export)]
pub enum UserShapeKind {
    /// The box tool: a tidy rectangle.
    Box { rect: Rect },
    /// The arrow tool, from `start` to `end`. An end that landed on a shape connects to it
    /// (`from`, `to`) and sits on that shape's outline.
    Arrow {
        start: Point,
        end: Point,
        from: Option<ElementId>,
        to: Option<ElementId>,
    },
    /// The pen: freehand strokes exactly as drawn. Strokes drawn one after another that overlap
    /// make one shape (a cylinder's body and its lid); `bounds` covers them all.
    Ink {
        strokes: Vec<Vec<Point>>,
        bounds: Rect,
    },
}

/// The hint shown while you speak a label: the words heard so far, and the shape they will
/// label.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LabelHint {
    pub target: ElementId,
    pub words: String,
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
                    fill: Some(super::Fill::Sky),
                    link: None,
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
