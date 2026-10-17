//! The surfaces beside the canvas as the UI gets them (PRODUCT §7.2, LLD §3.7): code, compare,
//! recap and Doc cards, and the updates that move their highlights and marks.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::Mark;
use crate::ids::SurfaceId;

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
    /// A Doc card (PRODUCT §7.2, artboard 14b, D-257): a passage or a figure of a shared
    /// document, opened when the talker cites it and the citation checks out against the
    /// document's digest or the pages read this session (law 6, I17). It takes a column slot
    /// like the code card.
    Doc {
        id: SurfaceId,
        /// The document as the card names it: its digest's title, else its file name.
        title: String,
        /// The checked citation: "p. 3 · Figure 1", "pp. 3–4", "Table 2".
        chip: String,
        /// The figure being discussed, when the citation names one.
        figure: Option<DocFigure>,
        /// The passage, a sentence per entry: as extracted from the page, or as the digest gives
        /// it (a figure's description, a table's rows).
        sentences: Vec<String>,
        /// The sentence being explained, from 0; it follows the voice (`SurfaceUpdate::Sentence`).
        highlight: Option<u32>,
        /// Who read what, in one line: "Figure read by Gemini 3.8 Flash. Redrawn on the board
        /// from the digest."
        note: String,
    },
}

/// The figure a Doc card shows. The card never shows the figure's image (D-260): its caption,
/// and that it is redrawn on the board.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DocFigure {
    /// "Figure 1", "Table 2".
    pub label: String,
    pub caption: String,
    /// A model read the figure, so its description is the card's passage and its diagram is in
    /// the digest; `false` when only its caption was read (no model that reads files).
    pub read: bool,
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
    /// Move the Doc card's highlight to this sentence (from 0): the one the voice is explaining.
    Sentence { index: u32 },
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
