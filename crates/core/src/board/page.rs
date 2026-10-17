//! Pages (D-177): how the board starts fresh while nothing drawn ever moves (I6).
//!
//! The board only grows, so "clear the board" can't erase anything. A page turn retires the
//! main layer whole, as an earlier page, and a new main layer starts with an empty grid and no
//! sequence. Its id stays `LayerId::MAIN`, so pointing, focus, the camera and the UI's main board
//! keep meaning "the page in front of you". The retired page is kept for the session's export and
//! named in the talker's summary, and the event log replays the turn. Detours stay in the board's
//! layers, tagged with the page they opened on, so the export shows each page next to its
//! detours.
//!
//! A page with nothing drawn on it doesn't turn: "clear the board" on a clean board, or a
//! `<page>` right after the user asked for one, changes nothing but the page's title.

use crate::board::{Board, Layer};
use crate::ids::LayerId;
use crate::limits::PAGES_PER_SESSION_MAX;

/// A page turned away from: kept whole for the record, never drawn on again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page {
    /// From 1.
    pub number: u16,
    pub title: Option<String>,
    pub layer: Layer,
}

/// The earlier pages, oldest first, and the current page's number and title.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Pages {
    earlier: Vec<Page>,
    current: u16,
    title: Option<String>,
}

impl Default for Pages {
    fn default() -> Self {
        Self {
            earlier: Vec::new(),
            current: 1,
            title: None,
        }
    }
}

impl Pages {
    pub(super) fn current(&self) -> u16 {
        self.current
    }
}

/// Why a page turn changed nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageRefused {
    /// Nothing is drawn on the current page: it already is a fresh one.
    Empty,
    /// The session has used `PAGES_PER_SESSION_MAX` pages; the board stays as it is.
    Full,
}

impl Layer {
    /// The page it belongs to, from 1.
    #[must_use]
    pub fn page(&self) -> u16 {
        self.page
    }
}

impl Board {
    /// The current page's number, from 1.
    #[must_use]
    pub fn page_number(&self) -> u16 {
        self.pages.current
    }

    /// The current page's title, when the talker named it.
    #[must_use]
    pub fn page_title(&self) -> Option<&str> {
        self.pages.title.as_deref()
    }

    /// The pages turned away from, oldest first.
    #[must_use]
    pub fn earlier_pages(&self) -> &[Page] {
        &self.pages.earlier
    }

    /// Turns to a fresh, empty page called `title` and returns its number. The caller closes
    /// the open detours first, so the new page is in front. An empty page only takes the title.
    pub fn turn_page(&mut self, title: Option<String>) -> Result<u16, PageRefused> {
        let current = self.pages.current;
        let main = self
            .layer_mut(LayerId::MAIN)
            .map_err(|_| PageRefused::Empty)?;
        if main.bounds().is_none() {
            if title.is_some() {
                self.pages.title = title;
            }
            return Err(PageRefused::Empty);
        }
        if current >= PAGES_PER_SESSION_MAX {
            return Err(PageRefused::Full);
        }
        let number = current + 1;
        let old = std::mem::replace(main, Layer::new(LayerId::MAIN, "main".to_owned(), number));
        self.pages.earlier.push(Page {
            number: current,
            title: std::mem::replace(&mut self.pages.title, title),
            layer: old,
        });
        self.pages.current = number;
        Ok(number)
    }
}

#[cfg(test)]
mod tests {
    use super::PageRefused;
    use crate::board::{Board, NodeRequest};
    use crate::ids::{ElementId, LayerId, OpId};
    use crate::limits::PAGES_PER_SESSION_MAX;
    use crate::narration::ops::Placement;
    use crate::time::SessionTime;

    fn draw(board: &mut Board, id: &str, op: u32) {
        let id = ElementId::parse(id).unwrap_or_else(|| unreachable!());
        let node = NodeRequest {
            id: &id,
            label: id.as_str(),
            shape: None,
            placement: &Placement::Auto,
            stack: 1,
            fill: None,
        };
        assert!(board.place_node(LayerId::MAIN, OpId(op), node).is_ok());
        let _ = board.start(LayerId::MAIN, &id, SessionTime::ZERO, 100);
    }

    #[test]
    fn a_page_turn_keeps_the_old_page_whole_and_starts_empty() {
        let mut board = Board::new();
        assert_eq!(board.turn_page(None), Err(PageRefused::Empty));
        draw(&mut board, "api", 0);
        let before = board.layer(LayerId::MAIN).cloned();
        assert_eq!(board.turn_page(Some("DNS".to_owned())), Ok(2));
        assert_eq!(board.page_number(), 2);
        assert_eq!(board.page_title(), Some("DNS"));
        let earlier = board.earlier_pages();
        assert_eq!(earlier.len(), 1);
        assert_eq!(Some(&earlier[0].layer), before.as_ref(), "kept as it was");
        assert_eq!(earlier[0].title, None);
        let main = board.layer(LayerId::MAIN).expect("main");
        assert!(main.elements().is_empty());
        assert_eq!(main.page(), 2);
        // The same id draws again on the new page, at the grid's origin.
        draw(&mut board, "api", 1);
        let redrawn = board.layer(LayerId::MAIN).and_then(|main| main.bounds());
        assert_eq!(redrawn, before.and_then(|layer| layer.bounds()));
    }

    #[test]
    fn an_empty_page_only_takes_the_title_and_pages_are_bounded() {
        let mut board = Board::new();
        assert_eq!(
            board.turn_page(Some("Raft".to_owned())),
            Err(PageRefused::Empty)
        );
        assert_eq!(board.page_title(), Some("Raft"));
        for page in 2..=PAGES_PER_SESSION_MAX {
            draw(&mut board, "a", u32::from(page));
            assert_eq!(board.turn_page(None), Ok(page));
        }
        draw(&mut board, "a", 0);
        assert_eq!(board.turn_page(None), Err(PageRefused::Full));
        assert_eq!(board.page_number(), PAGES_PER_SESSION_MAX);
        assert!(
            board
                .layer(LayerId::MAIN)
                .is_some_and(|main| main.bounds().is_some())
        );
    }

    #[test]
    fn a_detour_knows_its_page() {
        let mut board = Board::new();
        draw(&mut board, "a", 0);
        let first = board.push_layer("why");
        assert!(board.turn_page(None).is_ok());
        let second = board.push_layer("how");
        let page_of = |id: Option<LayerId>| {
            id.and_then(|id| board.layer(id))
                .map(super::super::Layer::page)
        };
        assert_eq!(page_of(first), Some(1));
        assert_eq!(page_of(second), Some(2));
    }
}
