//! The macOS FFI leaf: the VoiceProcessingIO audio unit, device queries, microphone permission
//! (`AVAudioApplication`) and the system voice (`AVSpeechSynthesizer`). It is the only module allowed `unsafe` (AGENTS.md); every block
//! states its argument in a `// SAFETY:` comment, and everything above it is safe Rust.
//!
//! Bindings are the objc2 family (`objc2-audio-toolbox`, `objc2-core-audio`,
//! `objc2-core-audio-types`, `objc2-avf-audio`, with `objc2`, `objc2-foundation` and `block2`
//! already in the graph through Tauri): generated from Apple's SDK headers, maintained together,
//! and permissively licensed, so no hand-written `extern` blocks are needed.
#![allow(
    unsafe_code,
    reason = "the CoreAudio and AVFAudio FFI leaf; each block carries a SAFETY argument"
)]

mod device;
mod main_loop;
mod permission;
#[cfg(test)]
mod realtime_tests;
pub(crate) mod system_voice;
mod unit;

pub(crate) use device::input_device_names;
pub use main_loop::run_main_loop_until;
pub(crate) use permission::{microphone_access, request_microphone_access};
pub(crate) use unit::VoiceProcessingUnit;

/// What a CoreAudio call returned when it failed.
pub(crate) type OsStatus = i32;
