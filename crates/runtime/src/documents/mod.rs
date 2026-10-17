//! The document reader (D-186, D-253 to D-259): BoardAI's [`SourceReader`] for Markdown, text
//! and PDF.
//!
//! - **Text, on this Mac.** Markdown and text are read natively; a PDF's text comes from macOS
//!   PDFKit (`pdfkit`), read in an order rebuilt from where its lines sit, the same on every
//!   macOS (`order`, D-293), with headings, captions and tables found in it (`layer`). A scanned
//!   PDF has no text and is said to need a model that reads files.
//! - **Figures, never lost silently.** When the configured talker or scribe is an OpenRouter
//!   model that takes files, it reads the PDF a page range at a time (`files`); else the coding
//!   harness reads a private copy of it (`harness`, D-231); else the digest is the text's alone, its figures listed by
//!   their captions and unread (`local`). Every figure the text names is in the digest, read or
//!   not.
//! - **Pages on request.** The talker's page reads come from the same text layer, extracted once
//!   and kept for a few documents; while a document's whole text isn't kept, a page read
//!   extracts only its own pages, so it never waits on a long document.
//!
//! Nothing here decides anything about the conversation: the core reads the digest, says what
//! the tray shows and what the talker hears.

pub mod files;
mod harness;
pub mod layer;
pub mod local;
pub mod order;
pub mod pdfkit;
mod reader;

pub use reader::{DOCUMENT_READER_NAME, DocumentReader, ReaderConfig};
