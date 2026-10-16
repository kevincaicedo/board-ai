//! Surfaces beside the canvas (PRODUCT §7.2): code, compare and recap cards, at most two at once.
//!
//! A code card shows only lines the session actually read (law 6): they come from
//! `ToolOutput::File` results, never from the talker's narration. A pen mark on a code line
//! names text that is on that line; the core finds its columns, so the talker never counts.

use std::collections::{BTreeMap, VecDeque};

use crate::ids::SurfaceId;
use crate::limits::{
    CODE_CARD_LINES_MAX, READ_FILE_LINES_MAX, READ_FILES_MAX, SURFACES_BESIDE_CANVAS_MAX,
};
use crate::narration::ops::{AxisRef, CloseTarget, LineRange};
use crate::protocol::{CellMark, ColumnRange, Mark, Surface, SurfaceUpdate, UiEvent};

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

    /// The read file `path` names: the path itself, or the one read file it ends ("service.go"
    /// for "internal/user/service.go"). `None` when it names none, or more than one.
    #[must_use]
    pub fn resolve(&self, path: &str) -> Option<String> {
        let path = normalize(path);
        if self.files.contains_key(&path) {
            return Some(path);
        }
        let suffix = format!("/{path}");
        let mut matches = self.files.keys().filter(|read| read.ends_with(&suffix));
        let found = matches.next()?;
        matches.next().is_none().then(|| found.clone())
    }

    /// Whether `line` of the read file `path` was read.
    #[must_use]
    pub fn has_line(&self, path: &str, line: u32) -> bool {
        self.files
            .get(&normalize(path))
            .is_some_and(|file| file.contains_key(&line))
    }

    /// The read lines a card opened on `line` shows: up to `before` read lines above it, then
    /// as many as were read after, at most `count` in all.
    #[must_use]
    pub fn around(&self, path: &str, line: u32, before: u32, count: u32) -> Option<LineRange> {
        let file = self.files.get(&normalize(path))?;
        file.get(&line)?;
        let first = (1..=before)
            .map_while(|back| {
                line.checked_sub(back)
                    .filter(|line| file.contains_key(line))
            })
            .last()
            .unwrap_or(line);
        let last = (first..first.saturating_add(count))
            .take_while(|line| file.contains_key(line))
            .last()
            .unwrap_or(line);
        LineRange::new(first, last)
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
    /// The text to mark isn't on that line of the code card.
    NotOnLine {
        line: u32,
    },
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
    Recap,
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
        let (id, _) = self.shown_line(line)?;
        events.push(UiEvent::SurfaceUpdate {
            id,
            update: SurfaceUpdate::Highlight { line },
        });
        Ok(())
    }

    /// A pen mark under (or around) `code` on `line` of the newest code card; the whole line,
    /// indentation aside, when `code` is `None`. The exact text is tried first, then the same
    /// text ignoring ASCII case.
    pub fn mark_code(
        &mut self,
        line: u32,
        code: Option<&str>,
        mark: Mark,
        events: &mut Vec<UiEvent>,
    ) -> Result<(), SurfaceRefused> {
        let (id, text) = self.shown_line(line)?;
        let columns = columns_of(text, code).ok_or(SurfaceRefused::NotOnLine { line })?;
        events.push(UiEvent::SurfaceUpdate {
            id,
            update: SurfaceUpdate::Mark {
                line,
                columns,
                mark,
            },
        });
        Ok(())
    }

    /// The newest code card and the text of `line` on it.
    fn shown_line(&self, line: u32) -> Result<(SurfaceId, &str), SurfaceRefused> {
        let (id, first_line, lines) = self
            .open
            .iter()
            .rev()
            .find_map(|open| match &open.kind {
                OpenKind::Code {
                    first_line, lines, ..
                } => Some((open.id, *first_line, lines)),
                OpenKind::Compare { .. } | OpenKind::Recap => None,
            })
            .ok_or(SurfaceRefused::NoCodeCard)?;
        let text = line
            .checked_sub(first_line)
            .and_then(|index| usize::try_from(index).ok())
            .and_then(|index| lines.get(index))
            .ok_or(SurfaceRefused::LineNotShown { line })?;
        Ok((id, text))
    }

    pub fn open_compare(
        &mut self,
        options: Vec<String>,
        criteria: Vec<String>,
        title: Option<String>,
        events: &mut Vec<UiEvent>,
    ) {
        let id = self.allocate(events);
        events.push(UiEvent::SurfaceShow {
            surface: Surface::Compare {
                id,
                options: options.clone(),
                criteria: criteria.clone(),
                title,
            },
        });
        self.open.push_back(Open {
            id,
            kind: OpenKind::Compare { options, criteria },
        });
    }

    /// Fills a cell of the newest compare card. Rows are criteria and columns are options; a
    /// talker that named them the other way round (both by name) still lands in the right cell.
    pub fn fill_cell(
        &mut self,
        criterion: &AxisRef,
        option: &AxisRef,
        mark: CellMark,
        events: &mut Vec<UiEvent>,
    ) -> Result<(), SurfaceRefused> {
        let (id, options, criteria) = self
            .open
            .iter()
            .rev()
            .find_map(|open| match &open.kind {
                OpenKind::Compare { options, criteria } => Some((open.id, options, criteria)),
                OpenKind::Code { .. } | OpenKind::Recap => None,
            })
            .ok_or(SurfaceRefused::NoCompareCard)?;
        let straight = resolve(criterion, criteria).zip(resolve(option, options));
        let swapped = match (criterion, option) {
            (AxisRef::Name(_), AxisRef::Name(_)) => {
                resolve(option, criteria).zip(resolve(criterion, options))
            }
            (AxisRef::Index(_) | AxisRef::Name(_), AxisRef::Index(_) | AxisRef::Name(_)) => None,
        };
        let (row, col) = straight.or(swapped).ok_or(SurfaceRefused::NoSuchCell)?;
        events.push(UiEvent::SurfaceUpdate {
            id,
            update: SurfaceUpdate::Cell { row, col, mark },
        });
        Ok(())
    }

    pub fn open_recap(&mut self, bullets: Vec<String>, events: &mut Vec<UiEvent>) {
        let id = self.allocate(events);
        events.push(UiEvent::SurfaceShow {
            surface: Surface::Recap { id, bullets },
        });
        self.open.push_back(Open {
            id,
            kind: OpenKind::Recap,
        });
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
                            | (OpenKind::Recap, CloseTarget::Recap)
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
            OpenKind::Code { .. } | OpenKind::Compare { .. } | OpenKind::Recap => None,
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

/// Columns of `code` on `line`, counted in chars; the line without its indentation and trailing
/// space when `code` is `None`.
fn columns_of(line: &str, code: Option<&str>) -> Option<ColumnRange> {
    let (begin_byte, end_byte) = match code.map(str::trim) {
        None => {
            let begin = line.len() - line.trim_start().len();
            (begin, line.trim_end().len().max(begin))
        }
        Some("") => return None,
        Some(code) => {
            let begin = line.find(code).or_else(|| find_ignoring_case(line, code))?;
            (begin, begin + code.len())
        }
    };
    if begin_byte >= end_byte {
        return None;
    }
    let column = |byte: usize| u32::try_from(line[..byte].chars().count()).ok();
    Some(ColumnRange {
        begin: column(begin_byte)?,
        end: column(end_byte)?,
    })
}

/// Where `needle` starts in `haystack` ignoring ASCII case. Lowercasing ASCII keeps every byte
/// offset, so the match maps back onto the original line.
fn find_ignoring_case(haystack: &str, needle: &str) -> Option<usize> {
    let begin = haystack
        .to_ascii_lowercase()
        .find(&needle.to_ascii_lowercase())?;
    haystack
        .is_char_boundary(begin)
        .then_some(begin)
        .filter(|begin| haystack.is_char_boundary(begin + needle.len()))
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
    use super::{SurfaceRefused, Surfaces, columns_of};
    use crate::narration::ops::{AxisRef, CloseTarget, LineRange};
    use crate::protocol::{CellMark, ColumnRange, Mark, Surface, SurfaceUpdate, UiEvent};

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
            surfaces.open_compare(
                vec!["Redis".into()],
                vec!["Speed".into()],
                None,
                &mut events,
            );
        }
        let dismissed: Vec<_> = events
            .iter()
            .filter(|event| matches!(event, UiEvent::SurfaceDismiss { .. }))
            .collect();
        assert_eq!(dismissed.len(), 1);
        let cell = surfaces.fill_cell(
            &AxisRef::Index(0),
            &AxisRef::Name("redis".into()),
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

    /// Rows are criteria and columns are options (the canvas's Compare card), and names given
    /// the other way round still find their cell.
    #[test]
    fn compare_cells_index_criteria_by_row_and_options_by_column() {
        let mut surfaces = Surfaces::default();
        let mut events = Vec::new();
        surfaces.open_compare(
            vec!["Raft".into(), "Paxos".into(), "ZAB".into()],
            vec!["Easy to follow".into(), "Used by".into()],
            Some("Raft vs Paxos vs ZAB".into()),
            &mut events,
        );
        let name = |text: &str| AxisRef::Name(text.into());
        for (criterion, option) in [("used by", "zab"), ("ZAB", "Used by")] {
            let mark = CellMark::Text {
                text: "ZooKeeper".into(),
            };
            assert!(
                surfaces
                    .fill_cell(&name(criterion), &name(option), mark, &mut events)
                    .is_ok()
            );
            assert_eq!(
                events.last(),
                Some(&UiEvent::SurfaceUpdate {
                    id: crate::ids::SurfaceId(0),
                    update: SurfaceUpdate::Cell {
                        row: 1,
                        col: 2,
                        mark: CellMark::Text {
                            text: "ZooKeeper".into()
                        },
                    },
                })
            );
        }
        assert_eq!(
            surfaces.fill_cell(
                &AxisRef::Index(2),
                &AxisRef::Index(0),
                CellMark::No,
                &mut events
            ),
            Err(SurfaceRefused::NoSuchCell)
        );
    }

    /// Frame 5: the pen underlines `s.cache.Get(` on line 43; the core finds the columns.
    #[test]
    fn a_code_mark_finds_its_columns_on_the_line() {
        let line = "    if u, err := s.cache.Get(ctx, userKey(id)); err == nil {";
        assert_eq!(
            columns_of(line, Some("s.cache.Get(")),
            Some(ColumnRange { begin: 17, end: 29 })
        );
        assert_eq!(
            columns_of(line, Some("S.CACHE.GET")),
            Some(ColumnRange { begin: 17, end: 28 })
        );
        assert_eq!(
            columns_of(line, None),
            Some(ColumnRange { begin: 4, end: 60 })
        );
        assert_eq!(
            columns_of("    // café → señor", Some("señor")),
            Some(ColumnRange { begin: 14, end: 19 })
        );
        assert_eq!(columns_of(line, Some("redis.Get")), None);
        assert_eq!(columns_of("    ", None), None);
        let mut surfaces = Surfaces::default();
        let mut events = Vec::new();
        assert_eq!(
            surfaces.mark_code(43, None, Mark::Underline, &mut events),
            Err(SurfaceRefused::NoCodeCard)
        );
        surfaces.record_read(
            "service.go",
            42,
            &[String::from("func f() {"), line.to_owned()],
        );
        assert!(surfaces.open_code("service.go", None, &mut events).is_ok());
        assert!(
            surfaces
                .mark_code(43, Some("s.cache.Get("), Mark::Underline, &mut events)
                .is_ok()
        );
        assert_eq!(
            surfaces.mark_code(44, None, Mark::Underline, &mut events),
            Err(SurfaceRefused::LineNotShown { line: 44 })
        );
        assert_eq!(
            surfaces.mark_code(43, Some("redis"), Mark::Circle, &mut events),
            Err(SurfaceRefused::NotOnLine { line: 43 })
        );
    }

    #[test]
    fn a_recap_opens_and_closes_like_any_card() {
        let mut surfaces = Surfaces::default();
        let mut events = Vec::new();
        surfaces.open_recap(vec!["Reads try Redis first".into()], &mut events);
        assert!(matches!(
            events.last(),
            Some(UiEvent::SurfaceShow {
                surface: Surface::Recap { .. }
            })
        ));
        assert!(surfaces.close(CloseTarget::Recap, &mut events).is_ok());
        assert_eq!(
            surfaces.close(CloseTarget::Recap, &mut events),
            Err(SurfaceRefused::NothingToClose)
        );
    }
}
