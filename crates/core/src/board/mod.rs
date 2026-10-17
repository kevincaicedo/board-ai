//! The board (LLD §2.5): a stack of layers, each a list of elements with stable ids.
//!
//! An element enters the board when its op becomes due: layout places it (`Pending`), it starts
//! drawing at its slot (`Drawing`), and it is `Drawn` once its budget has passed. An interruption
//! freezes the stroke in flight (`Frozen`) and removes anything still `Pending`, which the user
//! never saw. A repeated id updates the element (a strike, or finishing a frozen stroke) and
//! never duplicates it. Layout places only new ids, so nothing on the board moves (I6); the
//! exceptions only grow: a group's box, to keep its children inside, and a lifeline, as messages
//! go down a sequence diagram.
//!
//! One op can make several elements (a row of cells and each cell, a tree node and the line from
//! its parent). They share the op, and move through the states together.

mod cells;
mod draw;
mod element;
pub mod excalidraw;
mod free;
mod layer;
pub mod layout;
mod lifecycle;
pub mod metrics;
pub mod page;
mod place;
mod route;
mod sequence;
pub mod staged;
pub mod summary;
mod tone;
mod tree;
pub mod user;

use crate::ids::{ElementId, LayerId};
use crate::limits::LAYERS_PER_SESSION_MAX;
use tone::Tones;

pub use draw::{AnnotationRequest, EdgeRequest, NodeRequest};
pub use element::{Element, ElementKind, ElementState, Geometry, Placed, Rejected};
pub use layer::Layer;
pub use user::UserMark;

use draw::{generated_id, node_draw};

/// A frozen stroke, as the interruption reports it to the talker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrozenOp {
    pub layer: LayerId,
    pub id: ElementId,
    pub label: String,
    pub progress_percent: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Board {
    /// This page's main layer, then every detour of the session as it opened (closed ones too).
    layers: Vec<Layer>,
    tones: Tones,
    /// The number of your last shape's id (`u3`); ids of yours never repeat in a session.
    next_user: u32,
    /// Earlier pages and the current page's number and title (D-177).
    pages: page::Pages,
}

impl Default for Board {
    fn default() -> Self {
        Self::new()
    }
}

impl Board {
    #[must_use]
    pub fn new() -> Self {
        Self {
            layers: vec![Layer::new(LayerId::MAIN, "main".to_owned(), 1)],
            tones: Tones::default(),
            next_user: 0,
            pages: page::Pages::default(),
        }
    }

    #[must_use]
    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    #[must_use]
    pub fn layer(&self, id: LayerId) -> Option<&Layer> {
        self.layers.get(usize::from(id.0))
    }

    fn layer_mut(&mut self, id: LayerId) -> Result<&mut Layer, Rejected> {
        self.layers
            .get_mut(usize::from(id.0))
            .ok_or(Rejected::LayerMissing)
    }

    /// Opens a detour layer; `None` when the session has used every layer id.
    pub fn push_layer(&mut self, title: &str) -> Option<LayerId> {
        if self.layers.len() >= LAYERS_PER_SESSION_MAX {
            return None;
        }
        let id = LayerId(u8::try_from(self.layers.len()).ok()?);
        let page = self.pages.current();
        self.layers.push(Layer::new(id, title.to_owned(), page));
        Some(id)
    }

    /// Closes a detour layer. It stays in the board for the session export.
    pub fn close_layer(&mut self, id: LayerId) {
        if let Ok(layer) = self.layer_mut(id) {
            layer.open = false;
        }
    }
}
