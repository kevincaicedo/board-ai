//! Store errors keep their category all the way to the caller (AGENTS.md, errors are values).
//! Details name vault-relative paths and never quote file contents, so no error can carry a
//! secret out of a file.

use std::fmt;

/// Why a store call failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreError {
    /// The vault path was rejected at startup.
    Vault(VaultError),
    /// A vault file operation failed. `path` is relative to the vault.
    Io { path: String, detail: String },
    /// The index (SQLite) failed. It is derived, so a rebuild recovers it.
    Index { detail: String },
    /// A memory that must never be kept arrived (LLD §2.8).
    Refused { reason: Refusal },
    /// No session is recording.
    NoSession,
    /// A session is already recording; it must end first.
    SessionActive,
    /// "Don't remember this session" was said: nothing more is written for it (I10).
    SessionForgotten,
    /// What was asked for is not there: a proposal, a session, a file.
    NotFound { what: String },
    /// A store limit was reached (LLD §5).
    Limit { what: &'static str, max: usize },
    /// The store actor has stopped: after shutdown, or after a broken invariant.
    Stopped,
}

/// Why a memory was refused. The policy should never send these; the store checks anyway.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The policy's `MemoryAction::Never`.
    NeverAction,
    /// Your voice is never stored, only the transcript (law 10).
    RawAudio,
    /// Secrets are never stored (law 10).
    Secret,
}

/// Why the configured vault path cannot be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VaultError {
    NotAbsolute,
    /// The path exists and is not a folder.
    NotAFolder,
    /// `/`, your home folder or one of its parents: "don't remember this session" deletes
    /// folders inside the vault, so the vault must be a folder of its own.
    TooBroad,
    /// The folder can't be created or written.
    Unwritable {
        detail: String,
    },
}

impl StoreError {
    pub(crate) fn io(path: &str, error: &std::io::Error) -> Self {
        Self::Io {
            path: path.to_owned(),
            detail: error.kind().to_string(),
        }
    }

    pub(crate) fn index(error: &rusqlite::Error) -> Self {
        Self::Index {
            detail: error.to_string(),
        }
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::index(&error)
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Vault(error) => write!(formatter, "the vault can't be used: {error}"),
            Self::Io { path, detail } => write!(formatter, "{path}: {detail}"),
            Self::Index { detail } => write!(formatter, "the index failed: {detail}"),
            Self::Refused { reason } => write!(formatter, "not stored: {reason}"),
            Self::NoSession => formatter.write_str("no session is recording"),
            Self::SessionActive => formatter.write_str("a session is already recording"),
            Self::SessionForgotten => {
                formatter.write_str("this session is not being remembered, as you asked")
            }
            Self::NotFound { what } => write!(formatter, "{what} was not found"),
            Self::Limit { what, max } => write!(formatter, "too many {what} (the limit is {max})"),
            Self::Stopped => formatter.write_str("the store has stopped"),
        }
    }
}

impl std::error::Error for StoreError {}

impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NeverAction => "the memory policy says never",
            Self::RawAudio => "your voice is never stored",
            Self::Secret => "secrets are never stored",
        })
    }
}

impl fmt::Display for VaultError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAbsolute => formatter.write_str("the path must be absolute"),
            Self::NotAFolder => formatter.write_str("the path is not a folder"),
            Self::TooBroad => {
                formatter.write_str("it must be a folder of its own, not / or your home folder")
            }
            Self::Unwritable { detail } => write!(formatter, "it can't be written: {detail}"),
        }
    }
}
