//! Documents' bounds in the core (D-186, D-254 to D-257, LLD §5), re-exported from
//! `crate::limits`: the digest's shape, the pages a session keeps, page citations and the Doc
//! card.

/// One line of a digest; a longer line is cut at a character and counted (`truncated`).
pub const DIGEST_LINE_MAX_BYTES: usize = 2 * 1024;

/// Sections a digest keeps; later ones are dropped and counted.
pub const DIGEST_SECTIONS_MAX: usize = 48;

/// One section's summary; past it the lines are dropped and the digest counts `truncated`.
pub const DIGEST_SUMMARY_MAX_BYTES: usize = 1_024;

/// Figures a digest keeps; later ones are dropped and counted, and the tray counts them unread.
pub const DIGEST_FIGURES_MAX: usize = 40;

/// What a figure shows, in words; past it the lines are dropped and counted.
pub const DIGEST_FIGURE_SHOWS_MAX_BYTES: usize = 1_200;

/// A figure's or a table's caption, and a section's title; cut, ending with "…".
pub const DIGEST_CAPTION_MAX_BYTES: usize = 320;

/// A figure's `<diagram>` block. Under the narration's `DIAGRAM_BLOCK_MAX_BYTES`, so a block a
/// digest keeps is always whole when it is staged (D-234); a longer one is dropped and counted.
pub const DIGEST_DIAGRAM_MAX_BYTES: usize = 8 * 1024;

/// Tables a digest keeps; later ones are dropped and counted.
pub const DIGEST_TABLES_MAX: usize = 24;

/// Rows of one table (its header and divider count); later rows are dropped and counted.
pub const DIGEST_TABLE_ROWS_MAX: usize = 24;

/// One table row; cut at a character and counted.
pub const DIGEST_TABLE_ROW_MAX_BYTES: usize = 400;

/// Claims a digest keeps; later ones are dropped and counted.
pub const DIGEST_CLAIMS_MAX: usize = 40;

/// One claim; cut, ending with "…".
pub const DIGEST_CLAIM_MAX_BYTES: usize = 400;

/// The name of who read the figures ("Gemini 3.8 Flash"); cut, ending with "…".
pub const DIGEST_READER_MAX_BYTES: usize = 60;

/// The highest page or line number a locator may name; a larger one is not a locator.
pub const DOCUMENT_LOCATION_MAX: u32 = 1_000_000;

/// Pages one document read may return to the talker (`read_file` on a PDF, D-256); a wider range
/// is narrowed to its first pages and the result says so.
pub const DOCUMENT_READ_PAGES_MAX: u32 = 4;

/// One page's text as the core keeps it (for checking citations and the Doc card's passage);
/// past it the page is cut at a character.
pub const DOCUMENT_PAGE_TEXT_MAX_BYTES: usize = 8 * 1024;

/// Pages of documents the session keeps the text of; past it the oldest read page is forgotten
/// (its citations no longer check out unless the digest covers them).
pub const DOCUMENT_PAGES_KEPT_MAX: usize = 48;

/// Document reads waiting for the runtime's answer; past it the oldest is answered "not read"
/// and its late answer is dropped.
pub const DOCUMENT_READS_WAITING_MAX: usize = 4;

/// Page citations one reply may show; later ones in the reply show nothing (the talker still
/// says them).
pub const PAGE_CITES_PER_REPLY_MAX: usize = 8;

/// Spoken cues waiting for the voice to reach them (page citations and the Doc card's sentence
/// follow); past it the oldest is dropped.
pub const DOCUMENT_CUES_WAITING_MAX: usize = 64;

/// Sentences a Doc card shows; the passage is cut to the sentences around the one explained.
pub const DOC_CARD_SENTENCES_MAX: usize = 8;

/// One sentence on a Doc card; cut, ending with "…".
pub const DOC_CARD_SENTENCE_MAX_BYTES: usize = 400;

/// Words between a figure (or a table) and its page that still make one citation ("Figure 1 on
/// page 3"); farther apart they are two.
pub const CITE_JOIN_WORDS_MAX: usize = 3;

/// Citations read out of one clause; later ones in it are not looked at.
pub const CITES_PER_CLAUSE_MAX: usize = 4;

/// A Doc card's title; cut, ending with "…".
pub const DOC_CARD_TITLE_MAX_BYTES: usize = 120;

/// Sections, figures or tables named when a read asks for a part the digest doesn't have; the
/// rest aren't listed.
pub const DOCUMENT_PARTS_LISTED_MAX: usize = 12;

/// Content words a spoken clause must share with a passage's sentence for the Doc card's
/// highlight to move there; fewer, and the highlight stays.
pub const DOC_SENTENCE_OVERLAP_MIN: usize = 2;
