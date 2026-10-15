//! Model files (LLD §3.4): downloaded on demand into the app's data folder, never bundled.
//!
//! - The size is known before downloading (the manifest), so the first-run screen can show it.
//! - Downloads resume: bytes go to `<file>.part`, and a later attempt asks for the rest with an
//!   HTTP `Range`. A server that ignores the range restarts the file from zero.
//! - Every file is SHA-256 verified (`ring`, already in the graph through rustls) before it is
//!   renamed into place, so a file under its final name is always a verified one.
//! - Progress is reported through a callback at most every [`MODEL_PROGRESS_STEP_BYTES`].

mod manifest;

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::header::{CONTENT_RANGE, RANGE};
use ring::digest::{Context, SHA256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub use manifest::{ModelFile, ModelId};

use crate::limits::{MODEL_DOWNLOAD_IDLE_TIMEOUT_MS, MODEL_PROGRESS_STEP_BYTES};
use crate::llm::bounded_detail;
use crate::llm::http::build_client;

/// The app's identifier (`crates/app/tauri.conf.json`), which names its data folder.
const APP_IDENTIFIER: &str = "ai.boardai.desktop";

/// Bytes read at a time when hashing a partial file before resuming it.
const HASH_READ_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelStatus {
    Missing {
        size_bytes: u64,
    },
    /// A download stopped part way; the next one resumes it.
    Partial {
        have_bytes: u64,
        size_bytes: u64,
    },
    Ready {
        path: PathBuf,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DownloadProgress {
    pub received_bytes: u64,
    pub total_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelError {
    /// Connect or read failure, or the server went silent; the partial file is kept.
    Network {
        detail: String,
    },
    /// The server answered with an error status.
    Http {
        status: u16,
    },
    /// The server sent more bytes than the manifest says, or a range we did not ask for.
    BadResponse {
        detail: String,
    },
    /// The file's digest is not the manifest's; the download is deleted.
    Corrupt {
        expected: String,
        actual: String,
    },
    Io {
        detail: String,
    },
}

impl fmt::Display for ModelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network { detail } => write!(formatter, "model download failed: {detail}"),
            Self::Http { status } => write!(formatter, "model server answered {status}"),
            Self::BadResponse { detail } => write!(formatter, "model server misbehaved: {detail}"),
            Self::Corrupt { expected, actual } => write!(
                formatter,
                "model file is corrupt: SHA-256 {actual}, expected {expected}"
            ),
            Self::Io { detail } => write!(formatter, "model file error: {detail}"),
        }
    }
}

impl std::error::Error for ModelError {}

fn io_error(error: &std::io::Error) -> ModelError {
    ModelError::Io {
        detail: bounded_detail(&error.to_string()),
    }
}

/// The models folder and its downloads.
#[derive(Clone, Debug)]
pub struct ModelStore {
    root: PathBuf,
    client: reqwest::Client,
}

impl ModelStore {
    pub fn new(root: PathBuf) -> Result<Self, ModelError> {
        let client = build_client().map_err(|error| ModelError::Network {
            detail: error.to_string(),
        })?;
        Ok(Self { root, client })
    }

    /// `~/Library/Application Support/ai.boardai.desktop/models`.
    #[must_use]
    pub fn default_root(home: &Path) -> PathBuf {
        home.join("Library/Application Support")
            .join(APP_IDENTIFIER)
            .join("models")
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where a file lives once downloaded.
    #[must_use]
    pub fn path(&self, file: &ModelFile<'_>) -> PathBuf {
        self.root.join(file.file_name)
    }

    fn partial_path(&self, file: &ModelFile<'_>) -> PathBuf {
        self.root.join(format!("{}.part", file.file_name))
    }

    /// What is on disk now. A file under its final name was verified when it was renamed.
    #[must_use]
    pub fn status(&self, file: &ModelFile<'_>) -> ModelStatus {
        let path = self.path(file);
        if std::fs::metadata(&path).is_ok_and(|meta| meta.len() == file.size_bytes) {
            return ModelStatus::Ready { path };
        }
        match std::fs::metadata(self.partial_path(file)) {
            Ok(meta) if meta.len() > 0 => ModelStatus::Partial {
                have_bytes: meta.len(),
                size_bytes: file.size_bytes,
            },
            Ok(_) | Err(_) => ModelStatus::Missing {
                size_bytes: file.size_bytes,
            },
        }
    }

    /// The file's path, downloading (or resuming) and verifying it first if needed. Dropping
    /// the future stops the download and keeps the partial file.
    pub async fn fetch(
        &self,
        file: &ModelFile<'_>,
        progress: &mut (dyn FnMut(DownloadProgress) + Send),
    ) -> Result<PathBuf, ModelError> {
        if let ModelStatus::Ready { path } = self.status(file) {
            return Ok(path);
        }
        tokio::fs::create_dir_all(&self.root)
            .await
            .map_err(|error| io_error(&error))?;
        let partial = self.partial_path(file);
        let (mut hasher, have) = hash_partial(&partial, file.size_bytes).await?;
        let mut writer = Download::open(&partial, have).await?;
        let response = self.request(file, have).await?;
        if response.status() != StatusCode::PARTIAL_CONTENT && writer.received > 0 {
            // The server sent the whole file: start over.
            writer = Download::open(&partial, 0).await?;
            hasher = Context::new(&SHA256);
        }
        if response.status() == StatusCode::RANGE_NOT_SATISFIABLE {
            if writer.received != file.size_bytes {
                drop(writer);
                let _ = tokio::fs::remove_file(&partial).await;
                return Err(ModelError::BadResponse {
                    detail: "the server cannot resume this download".to_owned(),
                });
            }
        } else {
            writer
                .receive(response, file, &mut hasher, progress)
                .await?;
        }
        if writer.received != file.size_bytes {
            return Err(ModelError::Network {
                detail: format!("got {} of {} bytes", writer.received, file.size_bytes),
            });
        }
        writer.finish().await?;
        verify(hasher, file, &partial).await?;
        let path = self.path(file);
        tokio::fs::rename(&partial, &path)
            .await
            .map_err(|error| io_error(&error))?;
        Ok(path)
    }

    async fn request(
        &self,
        file: &ModelFile<'_>,
        have: u64,
    ) -> Result<reqwest::Response, ModelError> {
        let mut request = self.client.get(file.url);
        if have > 0 {
            request = request.header(RANGE, format!("bytes={have}-"));
        }
        let response = request.send().await.map_err(network)?;
        let status = response.status();
        if status == StatusCode::PARTIAL_CONTENT {
            let start = response
                .headers()
                .get(CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                .and_then(range_start);
            if start != Some(have) {
                return Err(ModelError::BadResponse {
                    detail: "the server resumed from the wrong byte".to_owned(),
                });
            }
        } else if !(status.is_success() || status == StatusCode::RANGE_NOT_SATISFIABLE && have > 0)
        {
            return Err(ModelError::Http {
                status: status.as_u16(),
            });
        }
        Ok(response)
    }
}

/// The partial file's digest so far and its length; a partial longer than the file is
/// discarded.
async fn hash_partial(path: &Path, size_bytes: u64) -> Result<(Context, u64), ModelError> {
    let mut hasher = Context::new(&SHA256);
    let Ok(mut reader) = tokio::fs::File::open(path).await else {
        return Ok((hasher, 0));
    };
    let length = reader
        .metadata()
        .await
        .map_err(|error| io_error(&error))?
        .len();
    if length > size_bytes {
        drop(reader);
        tokio::fs::remove_file(path)
            .await
            .map_err(|error| io_error(&error))?;
        return Ok((Context::new(&SHA256), 0));
    }
    let mut buffer = vec![0_u8; HASH_READ_BYTES];
    loop {
        let read = reader
            .read(&mut buffer)
            .await
            .map_err(|error| io_error(&error))?;
        if read == 0 {
            return Ok((hasher, length));
        }
        hasher.update(&buffer[..read]);
    }
}

struct Download {
    file: tokio::fs::File,
    received: u64,
    reported: u64,
}

impl Download {
    /// Opens the partial file to append after `keep` bytes (0 truncates it).
    async fn open(path: &Path, keep: u64) -> Result<Self, ModelError> {
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(keep > 0)
            .write(true)
            .truncate(keep == 0)
            .open(path)
            .await
            .map_err(|error| io_error(&error))?;
        Ok(Self {
            file,
            received: keep,
            reported: keep,
        })
    }

    async fn receive(
        &mut self,
        mut response: reqwest::Response,
        model: &ModelFile<'_>,
        hasher: &mut Context,
        progress: &mut (dyn FnMut(DownloadProgress) + Send),
    ) -> Result<(), ModelError> {
        let idle = Duration::from_millis(MODEL_DOWNLOAD_IDLE_TIMEOUT_MS);
        loop {
            let chunk = tokio::time::timeout(idle, response.chunk())
                .await
                .map_err(|_| ModelError::Network {
                    detail: "the server went silent".to_owned(),
                })?
                .map_err(network)?;
            let Some(chunk) = chunk else {
                break;
            };
            self.received += chunk.len() as u64;
            if self.received > model.size_bytes {
                return Err(ModelError::BadResponse {
                    detail: "the server sent more bytes than the file has".to_owned(),
                });
            }
            hasher.update(&chunk);
            self.file
                .write_all(&chunk)
                .await
                .map_err(|error| io_error(&error))?;
            if self.received - self.reported >= MODEL_PROGRESS_STEP_BYTES {
                self.reported = self.received;
                progress(DownloadProgress {
                    received_bytes: self.received,
                    total_bytes: model.size_bytes,
                });
            }
        }
        progress(DownloadProgress {
            received_bytes: self.received,
            total_bytes: model.size_bytes,
        });
        Ok(())
    }

    async fn finish(mut self) -> Result<(), ModelError> {
        self.file.flush().await.map_err(|error| io_error(&error))?;
        self.file.sync_all().await.map_err(|error| io_error(&error))
    }
}

/// Compares the digest; a mismatch deletes the download so the next attempt starts clean.
async fn verify(hasher: Context, file: &ModelFile<'_>, partial: &Path) -> Result<(), ModelError> {
    let actual = hex(hasher.finish().as_ref());
    if actual == file.sha256 {
        return Ok(());
    }
    let _ = tokio::fs::remove_file(partial).await;
    Err(ModelError::Corrupt {
        expected: file.sha256.to_owned(),
        actual,
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The first byte of a `Content-Range: bytes <start>-<end>/<total>` header.
fn range_start(value: &str) -> Option<u64> {
    let range = value.trim().strip_prefix("bytes ")?;
    let (start, _) = range.split_once('-')?;
    start.trim().parse().ok()
}

fn network(error: reqwest::Error) -> ModelError {
    ModelError::Network {
        detail: bounded_detail(&error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::{hex, range_start};

    #[test]
    fn content_range_starts_are_parsed() {
        assert_eq!(range_start("bytes 100-199/200"), Some(100));
        assert_eq!(range_start("bytes */200"), None);
        assert_eq!(range_start("items 1-2/3"), None);
    }

    #[test]
    fn digests_print_as_lowercase_hex() {
        assert_eq!(hex(&[0, 15, 255]), "000fff");
    }
}
