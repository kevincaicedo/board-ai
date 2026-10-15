//! Surfaces beside the canvas (PRODUCT §7.2): code cards and compare cards, at most two at once.
//!
//! A code card shows only lines the session actually read (law 6): they come from
//! `ToolOutput::File` results, never from the talker's narration.

use std::collections::{BTreeMap, VecDeque};

use crate::ids::SurfaceId;
use crate::limits::{
    CODE_CARD_LINES_MAX, READ_FILE_LINES_MAX, READ_FILES_MAX, SURFACES_BESIDE_CANVAS_MAX,
};
use crate::narration::ops::{AxisRef, CloseTarget, LineRange};
use crate::protocol::{CellMark, Surface, SurfaceUpdate, UiEvent};

/// Lines of files read this session, by path. The least recently read file is forgotten first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReadFiles {
    files: BTreeMap<String, BTreeMap<u32, String>>,
    order: VecDeque<String>,
}

impl ReadFiles {
    pub fn record(&mut self, path: &str, first_line: u32, lines: &[String]) {
        let path = normalize(path);
        self.order.retain(|known| *known != path);
        self.order.push_back(path.clone());
        let file = self.files.entry(path).or_default();
        for (line, text) in (first_line..).zip(lines) {
            if file.len() >= READ_FILE_LINES_MAX && !file.contains_key(&line) {
                break;
            }
            file.insert(line, text.clone());
        }
        while self.order.len() > READ_FILES_MAX {
            if let Some(oldest) = self.order.pop_front() {
                self.files.remove(&oldest);
            }
        }
    }

    #[must_use]
    pub fn was_read(&self, path: &str) -> bool {
        self.files.contains_key(&normalize(path))
    }

    /// Contiguous read lines starting at the range's first line (or the first line read).
    fn snippet(&self, path: &str, range: Option<LineRange>) -> Option<(u32, Vec<String>)> {
        let file = self.files.get(&normalize(path))?;
        let first = match range {
            Some(range) => range.first(),
            None => *file.keys().next()?,
        };
        let wanted = range.map_or(CODE_CARD_LINES_MAX, |range| {
            usize::try_from(range.last() - range.first() + 1).unwrap_or(CODE_CARD_LINES_MAX)
        });
        let lines: Vec<String> = (first..)
            .take(wanted.min(CODE_CARD_LINES_MAX))
            .map_while(|line| file.get(&line).cloned())
            .collect();
        (!lines.is_empty()).then_some((first, lines))
    }
}

fn normalize(path: &str) -> String {
    path.trim().trim_start_matches("./").to_owned()
}

/// Why a surface op did nothing. The op is dropped and the talker is told.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SurfaceRefused {
    /// `code` for a file not read this session, or lines that weren't read.
    NotRead {
        path: String,
    },
    NoCodeCard,
    LineNotShown {
        line: u32,
    },
    NoCompareCard,
    NoSuchCell,
    NothingToClose,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum OpenKind {
    Code {
        path: String,
        first_line: u32,
        lines: Vec<String>,
    },
    Compare {
        options: Vec<String>,
        criteria: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Open {
    id: SurfaceId,
    kind: OpenKind,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Surfaces {
    open: VecDeque<Open>,
    next_id: u32,
    read: ReadFiles,
}

impl Surfaces {
    pub fn record_read(&mut self, path: &str, first_line: u32, lines: &[String]) {
        self.read.record(path, first_line, lines);
    }

    #[must_use]
    pub fn read_files(&self) -> &ReadFiles {
        &self.read
    }

    pub fn open_code(
        &mut self,
        path: &str,
        range: Option<LineRange>,
        events: &mut Vec<UiEvent>,
    ) -> Result<(), SurfaceRefused> {
        let (first_line, lines) =
            self.read
                .snippet(path, range)
                .ok_or_else(|| SurfaceRefused::NotRead {
                    path: path.to_owned(),
                })?;
        let id = self.allocate(events);
        let path = normalize(path);
        events.push(UiEvent::SurfaceShow {
            surface: Surface::Code {
                id,
                path: path.clone(),
                first_line,
                lines: lines.clone(),
                highlight: None,
            },
        });
        self.open.push_back(Open {
            id,
            kind: OpenKind::Code {
                path,
                first_line,
                lines,
            },
        });
        Ok(())
    }

    pub fn highlight(
        &mut self,
        line: u32,
        events: &mut Vec<UiEvent>,
    ) -> Result<(), SurfaceRefused> {
        let (id, first_line, count) = self
            .open
            .iter()
            .rev()
            .find_map(|open| match &open.kind {
                OpenKind::Code {
                    first_line, lines, ..
                } => Some((open.id, *first_line, lines.len())),
                OpenKind::Compare { .. } => None,
            })
            .ok_or(SurfaceRefused::NoCodeCard)?;
        let shown = u32::try_from(count).unwrap_or(u32::MAX);
        if line < first_line || line >= first_line.saturating_add(shown) {
            return Err(SurfaceRefused::LineNotShown { line });
        }
        events.push(UiEvent::SurfaceUpdate {
            id,
            update: SurfaceUpdate::Highlight { line },
        });
        Ok(())
    }

    pub fn open_compare(
        &mut self,
        options: Vec<String>,
        criteria: Vec<String>,
        events: &mut Vec<UiEvent>,
    ) {
        let id = self.allocate(events);
        events.push(UiEvent::SurfaceShow {
            surface: Surface::Compare {
                id,
                options: options.clone(),
                criteria: criteria.clone(),
            },
        });
        self.open.push_back(Open {
            id,
            kind: OpenKind::Compare { options, criteria },
        });
    }

    pub fn fill_cell(
        &mut self,
        row: &AxisRef,
        col: &AxisRef,
        mark: CellMark,
        events: &mut Vec<UiEvent>,
    ) -> Result<(), SurfaceRefused> {
        let (id, options, criteria) = self
            .open
            .iter()
            .rev()
            .find_map(|open| match &open.kind {
                OpenKind::Compare { options, criteria } => Some((open.id, options, criteria)),
                OpenKind::Code { .. } => None,
            })
            .ok_or(SurfaceRefused::NoCompareCard)?;
        let row = resolve(row, options).ok_or(SurfaceRefused::NoSuchCell)?;
        let col = resolve(col, criteria).ok_or(SurfaceRefused::NoSuchCell)?;
        events.push(UiEvent::SurfaceUpdate {
            id,
            update: SurfaceUpdate::Cell { row, col, mark },
        });
        Ok(())
    }

    pub fn close(
        &mut self,
        target: CloseTarget,
        events: &mut Vec<UiEvent>,
    ) -> Result<(), SurfaceRefused> {
        let before = self.open.len();
        match target {
            CloseTarget::Latest => {
                if let Some(open) = self.open.pop_back() {
                    events.push(UiEvent::SurfaceDismiss { id: open.id });
                }
            }
            CloseTarget::All => {
                for open in self.open.drain(..) {
                    events.push(UiEvent::SurfaceDismiss { id: open.id });
                }
            }
            CloseTarget::Code | CloseTarget::Compare | CloseTarget::Recap => {
                self.open.retain(|open| {
                    let matches = matches!(
                        (&open.kind, target),
                        (OpenKind::Code { .. }, CloseTarget::Code)
                            | (OpenKind::Compare { .. }, CloseTarget::Compare)
                    );
                    if matches {
                        events.push(UiEvent::SurfaceDismiss { id: open.id });
                    }
                    !matches
                });
            }
        }
        if self.open.len() == before {
            return Err(SurfaceRefused::NothingToClose);
        }
        Ok(())
    }

    /// The text of a line on an open code card, for describing what the user points at.
    #[must_use]
    pub fn code_line(&self, surface: SurfaceId, line: u32) -> Option<(&str, &str)> {
        self.open.iter().find_map(|open| match &open.kind {
            OpenKind::Code {
                path,
                first_line,
                lines,
            } if open.id == surface => {
                let index = usize::try_from(line.checked_sub(*first_line)?).ok()?;
                Some((path.as_str(), lines.get(index)?.as_str()))
            }
            OpenKind::Code { .. } | OpenKind::Compare { .. } => None,
        })
    }

    /// A new surface id; the oldest surface is dismissed when two are already open.
    fn allocate(&mut self, events: &mut Vec<UiEvent>) -> SurfaceId {
        while self.open.len() >= SURFACES_BESIDE_CANVAS_MAX {
            if let Some(oldest) = self.open.pop_front() {
                events.push(UiEvent::SurfaceDismiss { id: oldest.id });
            }
        }
        let id = SurfaceId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        id
    }
}

fn resolve(axis: &AxisRef, names: &[String]) -> Option<u8> {
    let index = match axis {
        AxisRef::Index(index) => usize::from(*index),
        AxisRef::Name(name) => names
            .iter()
            .position(|item| item.eq_ignore_ascii_case(name))?,
    };
    (index < names.len())
        .then(|| u8::try_from(index).ok())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::{SurfaceRefused, Surfaces};
    use crate::narration::ops::{AxisRef, CloseTarget, LineRange};
    use crate::protocol::{CellMark, Surface, UiEvent};

    fn lines(count: u32) -> Vec<String> {
        (1..=count).map(|line| format!("line {line}")).collect()
    }

    #[test]
    fn a_code_card_needs_a_read_file_and_shows_its_lines() {
        let mut surfaces = Surfaces::default();
        let mut events = Vec::new();
        assert_eq!(
            surfaces.open_code("service.go", None, &mut events),
            Err(SurfaceRefused::NotRead {
                path: "service.go".into()
            })
        );
        surfaces.record_read("./service.go", 40, &lines(20));
        let range = LineRange::new(42, 44);
        assert!(surfaces.open_code("service.go", range, &mut events).is_ok());
        let Some(UiEvent::SurfaceShow {
            surface: Surface::Code {
                first_line, lines, ..
            },
        }) = events.last()
        else {
            panic!("expected a code card: {events:?}");
        };
        assert_eq!((*first_line, lines.len()), (42, 3));
        assert!(surfaces.highlight(43, &mut events).is_ok());
        assert_eq!(
            surfaces.highlight(50, &mut events),
            Err(SurfaceRefused::LineNotShown { line: 50 })
        );
        let unread = LineRange::new(90, 95);
        assert!(
            surfaces
                .open_code("service.go", unread, &mut events)
                .is_err()
        );
    }

    #[test]
    fn a_third_surface_dismisses_the_oldest() {
        let mut surfaces = Surfaces::default();
        let mut events = Vec::new();
        for _ in 0..3 {
            surfaces.open_compare(vec!["Redis".into()], vec!["Speed".into()], &mut events);
        }
        let dismissed: Vec<_> = events
            .iter()
            .filter(|event| matches!(event, UiEvent::SurfaceDismiss { .. }))
            .collect();
        assert_eq!(dismissed.len(), 1);
        let cell = surfaces.fill_cell(
            &AxisRef::Name("redis".into()),
            &AxisRef::Index(0),
            CellMark::Yes,
            &mut events,
        );
        assert!(cell.is_ok());
        assert!(surfaces.close(CloseTarget::All, &mut events).is_ok());
        assert_eq!(
            surfaces.close(CloseTarget::Latest, &mut events),
            Err(SurfaceRefused::NothingToClose)
        );
    }
}
