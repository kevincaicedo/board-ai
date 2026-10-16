//! Camera follow (PRODUCT §7.1, LLD §2.5). Layout never moves what is drawn (I6), so when the
//! board outgrows the view the camera moves instead.
//!
//! Each layer gets its first frame with its first drawing, and the camera follows as the
//! drawing grows: the frame is the layer's drawn bounds plus [`CAMERA_MARGIN_UNITS`], re-sent
//! once the drawing has grown [`CAMERA_REFRAME_STEP_UNITS`] past it. A `focus` tag frames one
//! element and holds the camera while new strokes land inside that frame; a stroke outside it
//! means the explanation moved on, so the camera goes back to following the whole layer.

use std::collections::BTreeMap;

use crate::board::layout;
use crate::geometry::Rect;
use crate::ids::{ElementId, LayerId};
use crate::limits::{CAMERA_MARGIN_UNITS, CAMERA_REFRAME_STEP_UNITS};
use crate::protocol::UiEvent;

#[derive(Clone, Debug, PartialEq, Eq)]
struct View {
    frame: Rect,
    focus: Option<ElementId>,
}

/// The frame last sent for each layer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Cameras {
    views: BTreeMap<LayerId, View>,
}

impl Cameras {
    /// A stroke covering `stroke` starts on `layer`, whose drawing now covers `drawing`. Returns
    /// the frame to send, if the camera has to move.
    pub(super) fn follow(
        &mut self,
        layer: LayerId,
        drawing: Rect,
        stroke: Rect,
    ) -> Option<UiEvent> {
        let wanted = layout::inflate(drawing, CAMERA_MARGIN_UNITS, 0);
        if let Some(view) = self.views.get(&layer) {
            let hold = match view.focus {
                Some(_) => view.frame.contains(stroke),
                None => !grown_past(view.frame, wanted),
            };
            if hold {
                return None;
            }
        }
        self.views.insert(
            layer,
            View {
                frame: wanted,
                focus: None,
            },
        );
        Some(UiEvent::Camera {
            layer,
            frame: wanted,
            focus: None,
        })
    }

    /// A `focus` tag: frame the element and sharpen it.
    pub(super) fn focus(&mut self, layer: LayerId, target: &ElementId, bounds: Rect) -> UiEvent {
        let frame = layout::inflate(bounds, CAMERA_MARGIN_UNITS, 0);
        self.views.insert(
            layer,
            View {
                frame,
                focus: Some(target.clone()),
            },
        );
        UiEvent::Camera {
            layer,
            frame,
            focus: Some(target.clone()),
        }
    }
}

/// True when `wanted` reaches at least a reframe step past `frame` on some side.
fn grown_past(frame: Rect, wanted: Rect) -> bool {
    let step = CAMERA_REFRAME_STEP_UNITS;
    wanted.x <= frame.x - step
        || wanted.y <= frame.y - step
        || wanted.right() >= frame.right() + step
        || wanted.bottom() >= frame.bottom() + step
}

#[cfg(test)]
mod tests {
    use super::Cameras;
    use crate::geometry::Rect;
    use crate::ids::{ElementId, LayerId};
    use crate::limits::{CAMERA_MARGIN_UNITS, CAMERA_REFRAME_STEP_UNITS};
    use crate::protocol::UiEvent;

    fn rect(x: i32, y: i32, w: u32, h: u32) -> Rect {
        Rect { x, y, w, h }
    }

    fn frame(event: Option<UiEvent>) -> Option<Rect> {
        match event? {
            UiEvent::Camera { frame, .. } => Some(frame),
            UiEvent::Orb { .. }
            | UiEvent::Acknowledge
            | UiEvent::Caption { .. }
            | UiEvent::Thread { .. }
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
            | UiEvent::Suggestion { .. } => None,
        }
    }

    #[test]
    fn frames_the_first_drawing_and_follows_by_steps() {
        let mut cameras = Cameras::default();
        let main = LayerId::MAIN;
        let first = rect(0, 0, 160, 80);
        let margin = CAMERA_MARGIN_UNITS;
        assert_eq!(
            frame(cameras.follow(main, first, first)),
            Some(rect(-margin, -margin, 280, 200))
        );
        // A mark just past the box stays inside the margin: no move.
        let small = rect(0, 0, 160 + 10, 80 + 10);
        assert_eq!(
            frame(cameras.follow(main, small, rect(150, 70, 20, 20))),
            None
        );
        // Growth of a whole step moves the camera to the new bounds.
        let step = u32::try_from(CAMERA_REFRAME_STEP_UNITS).expect("small");
        let grown = rect(0, 0, 160 + step, 80);
        assert_eq!(
            frame(cameras.follow(main, grown, rect(160, 0, step, 80))),
            Some(rect(-margin, -margin, 280 + step, 200))
        );
        // Each layer has its own camera.
        assert!(cameras.follow(LayerId(1), first, first).is_some());
    }

    #[test]
    fn focus_holds_until_a_stroke_lands_outside_it() {
        let mut cameras = Cameras::default();
        let main = LayerId::MAIN;
        let lb = ElementId::parse("lb").expect("valid");
        let board = rect(0, 0, 900, 300);
        assert!(cameras.follow(main, board, board).is_some());
        let focused = cameras.focus(main, &lb, rect(400, 100, 160, 80));
        assert!(matches!(focused, UiEvent::Camera { focus: Some(_), .. }));
        // Growth elsewhere is ignored while the stroke is inside the focused frame.
        let wider = rect(0, 0, 1400, 300);
        assert_eq!(
            frame(cameras.follow(main, wider, rect(420, 120, 40, 20))),
            None
        );
        let outside = cameras.follow(main, wider, rect(1200, 100, 160, 80));
        assert!(matches!(outside, Some(UiEvent::Camera { focus: None, .. })));
    }
}
