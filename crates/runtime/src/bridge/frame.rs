//! One frame's worth of UI events, coalesced (LLD §3.1 Bridge).
//!
//! Within a batch, an event that only restates state replaces the earlier one (latest wins):
//! the orb, the thread, the camera of a layer, and a caption that grows a caption not yet
//! final. The replaced slot is left empty and the new event goes last, so the order of what
//! remains is the order the core produced it. Everything else is kept: drawing, freeze and
//! resume, layers, surfaces, chips, problems, final captions.

use boardai_core::ids::LayerId;
use boardai_core::protocol::{Speaker, UiBatch, UiEvent};
use boardai_core::time::SessionTime;

use crate::limits::BRIDGE_BATCH_EVENTS_MAX;

/// Which earlier event a new one replaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Key {
    Orb,
    Thread,
    Camera(LayerId),
    /// A caption not yet final, by speaker.
    Caption(Speaker),
    /// The Sources tray and the drop overlay are sent whole; the latest wins (D-184).
    Sources,
    SourceDrop,
}

/// What happened to the events pushed into a frame, for the bridge's counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameCounts {
    pub coalesced: u64,
    /// Batches sent early because a frame reached [`BRIDGE_BATCH_EVENTS_MAX`].
    pub split: u64,
}

#[derive(Debug, Default)]
pub(crate) struct Frame {
    /// The latest step time in the frame: no event is shown before the core produced it.
    at: Option<SessionTime>,
    slots: Vec<Option<UiEvent>>,
    live: usize,
    /// The slot of the latest event for each key; at most a handful of keys per frame.
    keys: Vec<(Key, usize)>,
    pub(crate) counts: FrameCounts,
}

impl Frame {
    pub(crate) fn is_empty(&self) -> bool {
        self.live == 0
    }

    /// Adds one step's events. A batch that reaches the cap is pushed to `full` at once. A new
    /// session never shares a batch with the last one: its time starts again from zero, so the
    /// held batch goes first, under its own stamp.
    pub(crate) fn push(&mut self, at: SessionTime, events: Vec<UiEvent>, full: &mut Vec<UiBatch>) {
        for event in events {
            if matches!(event, UiEvent::SessionStart { .. }) {
                full.extend(self.take());
            }
            self.at = Some(self.at.map_or(at, |earlier| earlier.max(at)));
            self.add(event);
            if self.live >= BRIDGE_BATCH_EVENTS_MAX {
                self.counts.split += 1;
                full.extend(self.take());
            }
        }
    }

    /// The frame's batch, if it holds anything; the frame starts empty again.
    pub(crate) fn take(&mut self) -> Option<UiBatch> {
        let at = self.at.take()?;
        self.keys.clear();
        self.live = 0;
        let events: Vec<UiEvent> = self.slots.drain(..).flatten().collect();
        (!events.is_empty()).then_some(UiBatch { at, events })
    }

    fn add(&mut self, event: UiEvent) {
        let key = key(&event);
        if let Some(key) = key
            && let Some(position) = self.keys.iter().position(|(held, _)| *held == key)
        {
            let (_, slot) = self.keys.swap_remove(position);
            if self.slots.get_mut(slot).and_then(Option::take).is_some() {
                self.live -= 1;
                self.counts.coalesced += 1;
            }
        }
        // A final caption ends its line: the next caption starts a new one and replaces nothing.
        let held = match &event {
            UiEvent::Caption { is_final: true, .. } => None,
            UiEvent::Caption { .. }
            | UiEvent::Orb { .. }
            | UiEvent::Acknowledge
            | UiEvent::Thread { .. }
            | UiEvent::OpStart { .. }
            | UiEvent::Freeze { .. }
            | UiEvent::Resume { .. }
            | UiEvent::LayerPush { .. }
            | UiEvent::LayerPop { .. }
            | UiEvent::Camera { .. }
            | UiEvent::SurfaceShow { .. }
            | UiEvent::SurfaceUpdate { .. }
            | UiEvent::SurfaceDismiss { .. }
            | UiEvent::Chip { .. }
            | UiEvent::Problem { .. }
            | UiEvent::Suggestion { .. }
            | UiEvent::SessionStart { .. }
            | UiEvent::ViewShow { .. }
            | UiEvent::ViewDismiss
            | UiEvent::OpenSettings
            | UiEvent::SessionEnd
            | UiEvent::DrawMode { .. }
            | UiEvent::UserShape { .. }
            | UiEvent::UserShapeRemove { .. }
            | UiEvent::LabelHint { .. }
            | UiEvent::PageTurn { .. }
            | UiEvent::Diagram { .. }
            | UiEvent::Sources { .. }
            | UiEvent::SourceDrop { .. } => key,
        };
        if let Some(key) = held {
            self.keys.push((key, self.slots.len()));
        }
        self.slots.push(Some(event));
        self.live += 1;
    }
}

/// The coalescing key of an event: `None` for events that are never replaced.
fn key(event: &UiEvent) -> Option<Key> {
    match event {
        UiEvent::Orb { .. } => Some(Key::Orb),
        UiEvent::Thread { .. } => Some(Key::Thread),
        UiEvent::Camera { layer, .. } => Some(Key::Camera(*layer)),
        UiEvent::Caption { who, .. } => Some(Key::Caption(*who)),
        UiEvent::Sources { .. } => Some(Key::Sources),
        UiEvent::SourceDrop { .. } => Some(Key::SourceDrop),
        UiEvent::Acknowledge
        | UiEvent::OpStart { .. }
        | UiEvent::Freeze { .. }
        | UiEvent::Resume { .. }
        | UiEvent::LayerPush { .. }
        | UiEvent::LayerPop { .. }
        | UiEvent::SurfaceShow { .. }
        | UiEvent::SurfaceUpdate { .. }
        | UiEvent::SurfaceDismiss { .. }
        | UiEvent::Chip { .. }
        | UiEvent::Problem { .. }
        | UiEvent::Suggestion { .. }
        | UiEvent::SessionStart { .. }
        | UiEvent::ViewShow { .. }
        | UiEvent::ViewDismiss
        | UiEvent::OpenSettings
        | UiEvent::SessionEnd
        // The user's shapes are few and each one matters; a hint changes at most per partial.
        | UiEvent::DrawMode { .. }
        | UiEvent::UserShape { .. }
        | UiEvent::UserShapeRemove { .. }
        | UiEvent::LabelHint { .. }
        | UiEvent::PageTurn { .. }
        | UiEvent::Diagram { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use boardai_core::geometry::Rect;
    use boardai_core::ids::{ElementId, LayerId, OpId};
    use boardai_core::protocol::{DrawOp, NodeShape, OrbState, Speaker, UiBatch, UiEvent};
    use boardai_core::time::SessionTime;

    use super::Frame;
    use crate::limits::BRIDGE_BATCH_EVENTS_MAX;

    fn at(ms: u64) -> SessionTime {
        SessionTime::from_ms(ms)
    }

    fn caption(text: &str, is_final: bool) -> UiEvent {
        UiEvent::Caption {
            who: Speaker::Assistant,
            text: text.to_owned(),
            is_final,
        }
    }

    fn orb(state: OrbState, level_percent: u8) -> UiEvent {
        UiEvent::Orb {
            state,
            level_percent,
        }
    }

    fn op(index: u32) -> UiEvent {
        let Some(id) = ElementId::parse(&format!("n{index}")) else {
            panic!("valid id");
        };
        UiEvent::OpStart {
            layer: LayerId::MAIN,
            op: DrawOp::Node {
                id,
                op: OpId(index),
                shape: NodeShape::Service,
                label: "Node".to_owned(),
                rect: Rect {
                    x: 0,
                    y: 0,
                    w: 10,
                    h: 10,
                },
                stack: 1,
                fill: None,
                link: None,
            },
            budget_ms: 400,
        }
    }

    fn take(frame: &mut Frame) -> UiBatch {
        let Some(batch) = frame.take() else {
            panic!("the frame holds a batch");
        };
        batch
    }

    #[test]
    fn captions_and_the_orb_coalesce_and_drawing_keeps_its_order() {
        let mut frame = Frame::default();
        let mut full = Vec::new();
        frame.push(
            at(100),
            vec![orb(OrbState::Speaking, 10), caption("Sure", false)],
            &mut full,
        );
        frame.push(at(104), vec![op(1), caption("Sure. It", false)], &mut full);
        frame.push(at(110), vec![orb(OrbState::Drawing, 40), op(2)], &mut full);
        let batch = take(&mut frame);
        assert!(full.is_empty());
        assert_eq!(batch.at, at(110), "stamped with the latest step");
        assert_eq!(
            batch.events,
            vec![
                op(1),
                caption("Sure. It", false),
                orb(OrbState::Drawing, 40),
                op(2)
            ]
        );
        assert_eq!(frame.counts.coalesced, 2);
        assert!(frame.take().is_none(), "the frame starts empty again");
    }

    /// A new session's time starts again from zero: it must not take the last session's
    /// stamp, nor coalesce with its orb.
    #[test]
    fn a_new_session_never_shares_a_batch_with_the_last() {
        let mut frame = Frame::default();
        let mut full = Vec::new();
        frame.push(at(1_800_000), vec![orb(OrbState::Idle, 0)], &mut full);
        let start = UiEvent::SessionStart {
            session: Some("2026-10-15-150000".to_owned()),
        };
        frame.push(
            at(0),
            vec![start.clone(), orb(OrbState::Idle, 0)],
            &mut full,
        );
        assert_eq!(full.len(), 1);
        assert_eq!(full[0].at, at(1_800_000));
        assert_eq!(full[0].events, vec![orb(OrbState::Idle, 0)]);
        let batch = take(&mut frame);
        assert_eq!(batch.at, at(0));
        assert_eq!(batch.events, vec![start, orb(OrbState::Idle, 0)]);
        assert_eq!(frame.counts.coalesced, 0);
    }

    #[test]
    fn a_final_caption_is_never_replaced() {
        let mut frame = Frame::default();
        let mut full = Vec::new();
        let events = vec![
            caption("Sure.", true),
            caption("It starts", false),
            caption("It starts in the browser,", false),
        ];
        frame.push(at(5), events, &mut full);
        assert_eq!(
            take(&mut frame).events,
            vec![
                caption("Sure.", true),
                caption("It starts in the browser,", false)
            ]
        );
    }

    #[test]
    fn the_camera_coalesces_per_layer() {
        let camera = |layer: u8, x: i32| UiEvent::Camera {
            layer: LayerId(layer),
            frame: Rect {
                x,
                y: 0,
                w: 10,
                h: 10,
            },
            focus: None,
            enlarge: false,
        };
        let mut frame = Frame::default();
        let mut full = Vec::new();
        frame.push(
            at(1),
            vec![camera(0, 1), camera(1, 1), camera(0, 2)],
            &mut full,
        );
        assert_eq!(take(&mut frame).events, vec![camera(1, 1), camera(0, 2)]);
    }

    /// Crossing the cap never drops a drawing event: the full batch goes out at once and the
    /// rest follow, in order and with the same stamp.
    #[test]
    fn a_frame_over_the_cap_splits_instead_of_dropping() {
        let mut frame = Frame::default();
        let mut full = Vec::new();
        let total = u32::try_from(BRIDGE_BATCH_EVENTS_MAX).unwrap_or(u32::MAX) + 10;
        let mut events: Vec<UiEvent> = (0..total).map(op).collect();
        events.push(orb(OrbState::Speaking, 1));
        events.push(orb(OrbState::Speaking, 2));
        frame.push(at(7), events, &mut full);
        assert_eq!(full.len(), 1);
        assert_eq!(full[0].events.len(), BRIDGE_BATCH_EVENTS_MAX);
        let rest = take(&mut frame);
        assert_eq!(rest.at, at(7));
        let drawn: Vec<UiEvent> = full[0]
            .events
            .iter()
            .chain(&rest.events)
            .filter(|event| matches!(event, UiEvent::OpStart { .. }))
            .cloned()
            .collect();
        assert_eq!(drawn, (0..total).map(op).collect::<Vec<_>>());
        assert_eq!(rest.events.last(), Some(&orb(OrbState::Speaking, 2)));
        assert_eq!(frame.counts.split, 1);
    }
}
