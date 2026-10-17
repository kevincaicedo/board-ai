//! The document reader's limits (owner: runtime::documents, D-186, D-253 to D-259),
//! re-exported from `crate::limits`.

/// One PDFKit run (`osascript`), from spawn to its last byte; past it the process is killed and
/// the document's text is reported unread ("BoardAI couldn't read its text in time").
pub const PDFKIT_TIMEOUT_MS: u64 = 30_000;

/// What one PDFKit run may print (the text mode's JSON, or a split's base64); past it the
/// process is killed and the run fails as too large.
pub const PDFKIT_OUTPUT_MAX_BYTES: usize = 48 * 1024 * 1024;

/// Pages whose text is extracted; later pages are counted but not read (the tray says so, and
/// a page read past it finds no text).
pub const DOCUMENT_PAGES_MAX: u32 = 600;

/// A Markdown or text document's lines kept as its text layer; later lines are not read and
/// the digest counts them.
pub const DOCUMENT_LINES_MAX: usize = 50_000;

/// A page's extracted text; past it the page is cut at a character.
pub const DOCUMENT_PAGE_MAX_BYTES: usize = 16 * 1024;

/// Text layers the reader keeps for page reads (one per document); past it the least recently
/// used goes, and a later page read extracts the document again.
pub const DOCUMENT_LAYERS_CACHED_MAX: usize = 8;

/// Pages one digest call sends to a model that reads files; a longer document is read a range
/// at a time.
pub const DIGEST_PAGES_PER_CALL: u32 = 12;

/// Digest calls for one document; pages past `DIGEST_PAGES_PER_CALL × DIGEST_CALLS_MAX` keep
/// their text-only digest (a longer document goes to the harness when there is one).
pub const DIGEST_CALLS_MAX: u32 = 4;

/// One digest call, from the request to the whole answer; past it the call fails as a timeout
/// and its pages keep their text-only digest.
pub const DIGEST_CALL_TIMEOUT_MS: u64 = 240_000;

/// The answer a digest call may write, in tokens (OpenRouter's `max_tokens`).
pub const DIGEST_CALL_MAX_TOKENS: u32 = 12_000;

/// A page range's PDF as one call sends it, before base64; a larger one is not sent and its
/// pages keep their text-only digest (PDFKit's split can grow a range well past its share).
pub const DIGEST_CALL_PDF_MAX_BYTES: usize = 24 * 1024 * 1024;

/// A digest call's response body; past it the call fails as `BadOutput`.
pub const DIGEST_CALL_RESPONSE_MAX_BYTES: usize = 1024 * 1024;

/// Words a digest answer is asked to stay under, for the whole document: each call gets its
/// share, so the merged digest fits `DOCUMENT_DIGEST_MAX_BYTES`.
pub const DIGEST_WORDS_MAX: u32 = 9_000;

/// The OpenRouter model listing that says whether a model reads files; past it the model is
/// taken not to (the harness, or the text-only digest, reads the document instead).
pub const MODEL_FILES_CHECK_TIMEOUT_MS: u64 = 5_000;
pub const MODEL_FILES_CHECK_MAX_BYTES: usize = 512 * 1024;

/// Lines at the top of a PDF's first page looked at for its title (the largest text there).
pub const PDF_TITLE_LINES_MAX: usize = 12;

/// A PDF line that is a heading is at most this long; longer, it is body text set large.
pub const PDF_HEADING_MAX_BYTES: usize = 120;

/// Rows taken under a PDF table's caption; later lines are the page's text.
pub const PDF_TABLE_ROWS_MAX: usize = 16;

/// Image links a Markdown document's text layer lists as figures; later ones are not listed.
pub const MARKDOWN_IMAGE_LINKS_MAX: usize = 32;

/// Sentences of a section's first paragraph its text-only summary takes.
pub const DIGEST_SUMMARY_SENTENCES: usize = 2;

/// Images a Markdown document's digest call sends (local files it links); later ones stay
/// unread.
pub const MARKDOWN_IMAGES_MAX: usize = 8;

/// One linked image; a larger one is not sent and stays unread.
pub const MARKDOWN_IMAGE_MAX_BYTES: u64 = 4 * 1024 * 1024;
