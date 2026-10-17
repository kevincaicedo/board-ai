//! Sources in the runtime (D-184, D-185): every path checked where it enters, the session's
//! registry of what is shared and cached, the resolver for names said by voice, freshness
//! stamps for cached briefs, where a brief's harness may read (D-226), and native reads of
//! cited lines. Each piece does I/O and decides nothing about the conversation: the core
//! decides (law 1).

mod citations;
mod desk;
mod reach;
mod reader;
mod resolve;
mod stamp;
mod validate;

pub use citations::{Checked, check_citations};
pub use desk::{CachedBrief, SourceDesk, Written};
pub use reach::reach;
pub use reader::{ReaderEvent, ReaderStream, SourceReader};
pub use resolve::{Found, resolve};
pub use stamp::{Stamp, stamp};
pub use validate::{Refusal, ValidSource, expanded_path, shown_path, validate, written_path};
