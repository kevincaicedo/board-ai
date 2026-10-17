//! The document reader (D-186, D-253 to D-259): BoardAI's [`SourceReader`] for Markdown, text
//! and PDF.
//!
//! - **Text, on this Mac.** Markdown and text are read natively; a PDF's text comes from macOS
//!   PDFKit (`pdfkit`), with headings, captions and tables found in it (`layer`). A scanned PDF
//!   has no text and is said to need a model that reads files.
//! - **Figures, never lost silently.** When the configured talker or scribe is an OpenRouter
//!   model that takes files, it reads the PDF a page range at a time (`files`); else the coding
//!   harness reads it (`harness`); else the digest is the text's alone, its figures listed by
//!   their captions and unread (`local`). Every figure the text names is in the digest, read or
//!   not.
//! - **Pages on request.** The talker's page reads come from the same text layer, kept for a few
//!   documents.
//!
//! Nothing here decides anything about the conversation: the core reads the digest, says what
//! the tray shows and what the talker hears.

pub mod files;
mod harness;
pub mod layer;
pub mod local;
pub mod pdfkit;
mod reader;

pub use reader::{DOCUMENT_READER_NAME, DocumentReader, ReaderConfig};
