//! BoardAI's core: every conversation decision, with no side effects.
//!
//! The core owns the conversation state machine, the narration parser, speech-to-stroke
//! scheduling, the board model and layout, the thread and its detours, and the memory policy.
//! It performs no I/O and never reads a clock, spawns a thread or uses randomness. The runtime
//! feeds it time and inputs and carries out the effects it returns. That is what makes whole
//! conversations testable in a simulator and sessions replayable (docs/architecture/LLD.md §2).
#![forbid(unsafe_code)]

pub mod effect;
pub mod geometry;
pub mod harness;
pub mod history;
pub mod ids;
pub mod input;
pub mod limits;
pub mod memory;
pub mod protocol;
pub mod time;
pub mod tools;
