//! A PDF's text, from macOS PDFKit (D-253): `/usr/bin/osascript -l JavaScript` runs the fixed
//! script `pdfkit.js`, so reading a PDF needs no crate and no `unsafe`, and PDF parsing runs in a
//! process of its own. Every run is bounded in time and output, killed when dropped, and its
//! JSON is parsed here, once, into types.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use super::order::{Bounds, LineBox, reading_order};
use crate::limits::{
    PDF_FONT_SIZE_MAX_POINTS, PDFKIT_ERROR_SHOWN_MAX_CHARS, PDFKIT_OUTPUT_MAX_BYTES,
    PDFKIT_TIMEOUT_MS,
};

/// The script, embedded: no file of it is ever written or read at run time.
const SCRIPT: &str = include_str!("pdfkit.js");

/// macOS's script runner; never looked up on `PATH`.
pub const OSASCRIPT: &str = "/usr/bin/osascript";

/// One line of a page in reading order (`order`), with the font at its first character.
#[derive(Clone, Debug, PartialEq)]
pub struct PdfLine {
    pub text: String,
    /// The font's size in points, to a tenth; 0 when PDFKit gave none.
    pub size: f32,
    pub bold: bool,
}

/// A PDF's text, page by page (at most the pages asked for), and its length.
#[derive(Clone, Debug, PartialEq)]
pub struct PdfText {
    pub page_count: u32,
    /// The page `pages[0]` is (1-based): 1 for the whole text, the first asked for a page read.
    pub first_page: u32,
    pub pages: Vec<Vec<PdfLine>>,
}

/// Why PDFKit gave no text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PdfFailure {
    /// Not a PDF PDFKit can open.
    Unreadable,
    /// Encrypted: it needs a password.
    Locked { page_count: u32 },
    /// A range it couldn't split.
    BadRange,
    /// It printed more than `PDFKIT_OUTPUT_MAX_BYTES`.
    TooLarge,
    /// It ran past `PDFKIT_TIMEOUT_MS`.
    TimedOut,
    /// It couldn't run, or answered something that isn't the script's JSON.
    Failed { detail: String },
}

impl PdfFailure {
    /// What the tray says.
    #[must_use]
    pub fn reason(&self) -> String {
        match self {
            Self::Unreadable => "BoardAI couldn't open it as a PDF".to_owned(),
            Self::Locked { .. } => "It is locked with a password".to_owned(),
            Self::BadRange => "BoardAI couldn't split its pages".to_owned(),
            Self::TooLarge => "Its text is too large to read".to_owned(),
            Self::TimedOut => "BoardAI couldn't read its text in time".to_owned(),
            Self::Failed { .. } => "BoardAI couldn't read its text".to_owned(),
        }
    }
}

/// Runs the PDFKit script. `program` is [`OSASCRIPT`]; tests point it at a stand-in.
#[derive(Clone, Debug)]
pub struct PdfKit {
    program: PathBuf,
    timeout: Duration,
}

impl Default for PdfKit {
    fn default() -> Self {
        Self {
            program: PathBuf::from(OSASCRIPT),
            timeout: Duration::from_millis(PDFKIT_TIMEOUT_MS),
        }
    }
}

#[derive(Deserialize)]
struct RawText {
    #[serde(default)]
    page_count: f64,
    /// A page read's first page; absent for the whole text.
    #[serde(default)]
    first: Option<f64>,
    #[serde(default)]
    locked: bool,
    #[serde(default)]
    pages: Vec<RawPage>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct RawPage {
    lines: Vec<RawLine>,
}

#[derive(Deserialize)]
struct RawLine {
    text: String,
    #[serde(default)]
    size: f64,
    #[serde(default)]
    bold: bool,
    /// Where PDFKit laid the line out: `[x, y, width, height]` in PDF points.
    #[serde(rename = "box")]
    bounds: [f64; 4],
}

impl PdfKit {
    /// A runner with another program and deadline, for tests.
    #[must_use]
    pub fn with_program(program: PathBuf, timeout: Duration) -> Self {
        Self { program, timeout }
    }

    /// The text of the first `max_pages` pages of the PDF at `path` (absolute, validated).
    pub async fn text(&self, path: &Path, max_pages: u32) -> Result<PdfText, PdfFailure> {
        let output = self
            .run([
                OsString::from("text"),
                path.as_os_str().to_owned(),
                OsString::from(max_pages.to_string()),
            ])
            .await?;
        parse_text(&output, 1, max_pages)
    }

    /// The text of pages `[first, last]` (1-based) alone, for a page read (D-256): PDFKit opens
    /// the document and extracts only those, so a page of a long PDF is read in a fraction of a
    /// second. Pages past the end are left out.
    pub async fn pages(&self, path: &Path, first: u32, last: u32) -> Result<PdfText, PdfFailure> {
        if first == 0 || last < first {
            return Err(PdfFailure::BadRange);
        }
        let output = self
            .run([
                OsString::from("pages"),
                path.as_os_str().to_owned(),
                OsString::from(first.to_string()),
                OsString::from(last.to_string()),
            ])
            .await?;
        let text = parse_text(&output, first, last - first + 1)?;
        if text.first_page != first {
            return Err(PdfFailure::Failed {
                detail: "its answer was for other pages".to_owned(),
            });
        }
        Ok(text)
    }

    /// Pages `[first, last]` (1-based) of the PDF at `path` as a PDF of their own, in base64.
    pub async fn split(&self, path: &Path, first: u32, last: u32) -> Result<String, PdfFailure> {
        let output = self
            .run([
                OsString::from("split"),
                path.as_os_str().to_owned(),
                OsString::from(first.to_string()),
                OsString::from(last.to_string()),
            ])
            .await?;
        let text = String::from_utf8(output).map_err(|_| PdfFailure::Failed {
            detail: "its answer wasn't text".to_owned(),
        })?;
        let text = text.trim();
        if text.starts_with('{') {
            let raw: RawText = serde_json::from_str(text).map_err(|error| PdfFailure::Failed {
                detail: format!("its answer wasn't the script's JSON: {error}"),
            })?;
            return Err(failure(raw.error.as_deref().unwrap_or("unreadable"), 0));
        }
        let base64 = text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='));
        if text.is_empty() || !base64 {
            return Err(PdfFailure::Failed {
                detail: "its answer wasn't base64".to_owned(),
            });
        }
        Ok(text.to_owned())
    }

    /// One bounded run: stdout read up to the bound, stderr (where PDFKit logs) dropped, the
    /// process killed when it runs past the deadline or this future is dropped.
    async fn run<const N: usize>(&self, arguments: [OsString; N]) -> Result<Vec<u8>, PdfFailure> {
        let mut command = Command::new(&self.program);
        command
            .args(["-l", "JavaScript", "-e", SCRIPT])
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|error| PdfFailure::Failed {
            detail: format!("osascript didn't start: {error}"),
        })?;
        let Some(stdout) = child.stdout.take() else {
            return Err(PdfFailure::Failed {
                detail: "osascript had no output".to_owned(),
            });
        };
        let read = async move {
            let mut output = Vec::new();
            let limit = u64::try_from(PDFKIT_OUTPUT_MAX_BYTES).unwrap_or(u64::MAX) + 1;
            let mut bounded = stdout.take(limit);
            bounded
                .read_to_end(&mut output)
                .await
                .map_err(|error| PdfFailure::Failed {
                    detail: format!("its output couldn't be read: {error}"),
                })?;
            if output.len() > PDFKIT_OUTPUT_MAX_BYTES {
                return Err(PdfFailure::TooLarge);
            }
            let status = child.wait().await.map_err(|error| PdfFailure::Failed {
                detail: format!("osascript didn't end: {error}"),
            })?;
            if !status.success() {
                return Err(PdfFailure::Failed {
                    detail: format!("osascript ended with {status}"),
                });
            }
            Ok(output)
        };
        tokio::time::timeout(self.timeout, read)
            .await
            .unwrap_or(Err(PdfFailure::TimedOut))
    }
}

/// The script's `text` or `pages` JSON, parsed once into types: at most `max_pages` pages, each
/// line's font size to a tenth of a point, each page's lines in reading order (D-293).
fn parse_text(output: &[u8], first_page: u32, max_pages: u32) -> Result<PdfText, PdfFailure> {
    let raw: RawText = serde_json::from_slice(output).map_err(|error| PdfFailure::Failed {
        detail: format!("its answer wasn't the script's JSON: {error}"),
    })?;
    let page_count = number(raw.page_count);
    if let Some(error) = raw.error {
        return Err(failure(&error, page_count));
    }
    if raw.locked {
        return Err(PdfFailure::Locked { page_count });
    }
    let pages = raw
        .pages
        .into_iter()
        .take(usize::try_from(max_pages).unwrap_or(usize::MAX))
        .map(|page| reading_order(page.lines.into_iter().filter_map(line_box).collect()))
        .collect();
    Ok(PdfText {
        page_count,
        first_page: raw.first.map_or(first_page, number),
        pages,
    })
}

/// A line the script sent, with its box; `None` when it is blank.
fn line_box(line: RawLine) -> Option<LineBox> {
    let [x, y, width, height] = line.bounds;
    LineBox::new(
        PdfLine {
            text: line.text,
            // A tenth of a point is all heading detection needs.
            #[allow(clippy::cast_possible_truncation, reason = "font sizes are small")]
            size: (line.size.clamp(0.0, PDF_FONT_SIZE_MAX_POINTS) * 10.0).round() as f32 / 10.0,
            bold: line.bold,
        },
        Bounds {
            x,
            y,
            width,
            height,
        },
    )
}

fn failure(error: &str, page_count: u32) -> PdfFailure {
    match error {
        "locked" => PdfFailure::Locked { page_count },
        "bad range" => PdfFailure::BadRange,
        "unreadable" => PdfFailure::Unreadable,
        other => PdfFailure::Failed {
            detail: format!(
                "the script said {:?}",
                other
                    .chars()
                    .take(PDFKIT_ERROR_SHOWN_MAX_CHARS)
                    .collect::<String>()
            ),
        },
    }
}

/// A count the script sent as a JSON number.
fn number(value: f64) -> u32 {
    if value.is_finite() && value >= 0.0 {
        #[allow(clippy::cast_possible_truncation, reason = "clamped to u32 first")]
        #[allow(clippy::cast_sign_loss, reason = "non-negative")]
        let count = value.min(f64::from(u32::MAX)) as u32;
        count
    } else {
        0
    }
}
