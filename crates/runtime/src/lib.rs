//! BoardAI's runtime: all I/O, and no conversation decisions.
//!
//! The runtime owns audio capture and playback, voice detection, speech-to-text,
//! text-to-speech, the LLM providers, repo tools, coding harness subprocesses and the store. One
//! orchestrator task owns the core: it steps the core with inputs and carries out the effects
//! the core returns. The runtime never imports Tauri, so the same code can run behind a
//! WebSocket for the hosted version (docs/architecture/LLD.md §3).
//!
//! `unsafe` is denied here and allowed only in the CoreAudio FFI module (`audio::coreaudio`),
//! which carries a module-level allow and a `// SAFETY:` argument on every block.
#![deny(unsafe_code)]

pub mod audio;
pub mod bridge;
pub mod cancel;
pub mod clock;
pub mod documents;
pub mod harness;
pub mod limits;
pub mod llm;
pub mod logging;
pub mod models;
pub mod session;
pub mod settings;
pub mod sources;
pub mod speech_in;
pub mod speech_out;
pub mod store;
pub mod tools;
