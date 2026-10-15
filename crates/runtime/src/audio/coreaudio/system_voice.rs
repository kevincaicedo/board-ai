//! The macOS system voice: `AVSpeechSynthesizer` writing to buffers with word markers (macOS
//! 14+). No download, exact word timings.
//!
//! One thread owns the synthesizer (it is not `Send`) and speaks one request at a time. Apple
//! delivers the buffers and markers on the main dispatch queue (measured: whichever thread asked,
//! the callbacks ran on `main`), so the blocks only copy what they get into a channel; they never
//! wait. Measured on macOS 26: the default en-US voice writes 22.05 kHz mono `Float32`, and a
//! marker's `byteSampleOffset` counts bytes of that output (4 per frame), not frames.

use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2_avf_audio::{
    AVAudioBuffer, AVAudioCommonFormat, AVSpeechBoundary, AVSpeechSynthesisMarker,
    AVSpeechSynthesisMarkerMark, AVSpeechSynthesisVoice, AVSpeechSynthesizer, AVSpeechUtterance,
};
use objc2_core_audio_types::AudioBufferList;
use objc2_foundation::{NSArray, NSString};
use tokio::sync::mpsc;

use crate::cancel::CancelToken;
use crate::limits::{SYSTEM_VOICE_FIRST_AUDIO_TIMEOUT_MS, TTS_CLAUSE_TIMEOUT_MS};

/// How often the owner thread checks for completion and cancellation.
const POLL: Duration = Duration::from_millis(5);

/// What the synthesizer reports for one request, in order.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum SpeechEvent {
    /// Mono samples at `rate_hz` (channel 0 when the voice writes more than one).
    Audio {
        samples: Vec<f32>,
        rate_hz: f64,
        bytes_per_frame: u32,
    },
    /// A word starts at `byte_offset` bytes into the output, at `utf16_index` in the text.
    Word {
        utf16_index: usize,
        byte_offset: u64,
    },
    Finished,
    Failed(SystemVoiceFailure),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SystemVoiceFailure {
    /// No voice has this identifier.
    UnknownVoice,
    /// No audio arrived in time. Usually the main thread runs no event loop, so the callbacks
    /// are never delivered.
    NoAudio,
    /// The voice took longer than the clause timeout.
    TimedOut,
    /// A buffer in a sample format we do not read.
    UnsupportedFormat,
    /// The owner thread is gone.
    Stopped,
}

/// A voice that can be chosen in settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemVoiceInfo {
    pub identifier: String,
    pub name: String,
    pub language: String,
    /// 1 default, 2 enhanced, 3 premium (Apple's `AVSpeechSynthesisVoiceQuality`).
    pub quality: i64,
}

struct Request {
    text: String,
    voice: Option<String>,
    events: mpsc::UnboundedSender<SpeechEvent>,
    cancel: CancelToken,
}

/// Owns the synthesizer thread. Dropping it ends the thread after the current request.
pub(crate) struct SystemSynthesizer {
    requests: Option<std::sync::mpsc::Sender<Request>>,
    thread: Option<JoinHandle<()>>,
}

impl SystemSynthesizer {
    pub(crate) fn spawn() -> std::io::Result<Self> {
        let (requests, receiver) = std::sync::mpsc::channel::<Request>();
        let thread = std::thread::Builder::new()
            .name("system-voice".to_owned())
            .spawn(move || {
                // SAFETY: creating a synthesizer has no preconditions; it stays on this thread.
                let synthesizer = unsafe { AVSpeechSynthesizer::new() };
                while let Ok(request) = receiver.recv() {
                    speak(&synthesizer, request);
                }
            })?;
        Ok(Self {
            requests: Some(requests),
            thread: Some(thread),
        })
    }

    /// Queues one utterance. The events arrive on the returned receiver; the channel is bounded
    /// by the clause (one clause is at most a few hundred bytes of text).
    pub(crate) fn speak(
        &self,
        text: String,
        voice: Option<String>,
        cancel: CancelToken,
    ) -> mpsc::UnboundedReceiver<SpeechEvent> {
        let (events, receiver) = mpsc::unbounded_channel();
        let request = Request {
            text,
            voice,
            events: events.clone(),
            cancel,
        };
        let sent = self
            .requests
            .as_ref()
            .is_some_and(|requests| requests.send(request).is_ok());
        if !sent {
            let _ = events.send(SpeechEvent::Failed(SystemVoiceFailure::Stopped));
        }
        receiver
    }
}

impl Drop for SystemSynthesizer {
    fn drop(&mut self) {
        self.requests = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The installed voices for `language` (for example `en-US`), best quality first.
pub(crate) fn voices(language: &str) -> Vec<SystemVoiceInfo> {
    // SAFETY: a class method with no preconditions; it returns an owned array of voices.
    let all = unsafe { AVSpeechSynthesisVoice::speechVoices() };
    let mut voices: Vec<SystemVoiceInfo> = all
        .iter()
        .map(|voice| {
            // SAFETY: plain property reads on a live voice object.
            unsafe {
                SystemVoiceInfo {
                    identifier: voice.identifier().to_string(),
                    name: voice.name().to_string(),
                    language: voice.language().to_string(),
                    quality: voice.quality().0 as i64,
                }
            }
        })
        .filter(|voice| voice.language == language)
        .collect();
    voices.sort_by(|a, b| b.quality.cmp(&a.quality).then(a.name.cmp(&b.name)));
    voices
}

fn speak(synthesizer: &AVSpeechSynthesizer, request: Request) {
    let Request {
        text,
        voice,
        events,
        cancel,
    } = request;
    let Some(utterance) = utterance(&text, voice.as_deref()) else {
        let _ = events.send(SpeechEvent::Failed(SystemVoiceFailure::UnknownVoice));
        return;
    };
    let progress = Arc::new(Progress::default());
    let buffer_block = buffer_block(events.clone(), Arc::clone(&progress));
    let marker_block = marker_block(events.clone());
    // SAFETY: both blocks are live `RcBlock`s; the synthesizer copies them, and their closures
    // capture only `Send + Sync` values (a channel sender and atomics), so running them on the
    // main queue is sound.
    unsafe {
        synthesizer.writeUtterance_toBufferCallback_toMarkerCallback(
            &utterance,
            RcBlock::as_ptr(&buffer_block),
            RcBlock::as_ptr(&marker_block),
        );
    }
    if let Some(failure) = wait(&progress, &cancel) {
        // SAFETY: stopping a live synthesizer has no preconditions.
        unsafe {
            synthesizer.stopSpeakingAtBoundary(AVSpeechBoundary::Immediate);
        }
        let _ = events.send(SpeechEvent::Failed(failure));
    }
}

#[derive(Debug, Default)]
struct Progress {
    got_audio: AtomicBool,
    finished: AtomicBool,
    failed: AtomicBool,
}

/// Waits for the end of the utterance. `Some` is a failure to report; a cancellation ends the
/// wait quietly (the receiver is gone or about to be).
fn wait(progress: &Progress, cancel: &CancelToken) -> Option<SystemVoiceFailure> {
    let started = Instant::now();
    loop {
        if progress.finished.load(Ordering::Acquire) {
            return None;
        }
        if progress.failed.load(Ordering::Acquire) {
            return Some(SystemVoiceFailure::UnsupportedFormat);
        }
        if cancel.is_cancelled() {
            return None;
        }
        let elapsed = started.elapsed();
        if !progress.got_audio.load(Ordering::Acquire)
            && elapsed > Duration::from_millis(SYSTEM_VOICE_FIRST_AUDIO_TIMEOUT_MS)
        {
            return Some(SystemVoiceFailure::NoAudio);
        }
        if elapsed > Duration::from_millis(TTS_CLAUSE_TIMEOUT_MS) {
            return Some(SystemVoiceFailure::TimedOut);
        }
        std::thread::sleep(POLL);
    }
}

fn utterance(text: &str, voice: Option<&str>) -> Option<Retained<AVSpeechUtterance>> {
    // SAFETY: class methods with no preconditions, given live strings.
    unsafe {
        let utterance = AVSpeechUtterance::speechUtteranceWithString(&NSString::from_str(text));
        let chosen = match voice {
            Some(identifier) => Some(AVSpeechSynthesisVoice::voiceWithIdentifier(
                &NSString::from_str(identifier),
            )?),
            None => AVSpeechSynthesisVoice::voiceWithLanguage(Some(&NSString::from_str("en-US"))),
        };
        utterance.setVoice(chosen.as_deref());
        Some(utterance)
    }
}

type BufferBlock = RcBlock<dyn Fn(NonNull<AVAudioBuffer>)>;
type MarkerBlock = RcBlock<dyn Fn(NonNull<NSArray<AVSpeechSynthesisMarker>>)>;

fn buffer_block(
    events: mpsc::UnboundedSender<SpeechEvent>,
    progress: Arc<Progress>,
) -> BufferBlock {
    RcBlock::new(move |buffer: NonNull<AVAudioBuffer>| {
        // SAFETY: AVFAudio passes a live buffer for the duration of the call.
        let buffer = unsafe { buffer.as_ref() };
        match read_buffer(buffer) {
            Some(event @ SpeechEvent::Audio { .. }) => {
                progress.got_audio.store(true, Ordering::Release);
                let _ = events.send(event);
            }
            Some(SpeechEvent::Finished) => {
                let _ = events.send(SpeechEvent::Finished);
                progress.finished.store(true, Ordering::Release);
            }
            Some(SpeechEvent::Word { .. } | SpeechEvent::Failed(_)) | None => {
                progress.failed.store(true, Ordering::Release);
            }
        }
    })
}

fn marker_block(events: mpsc::UnboundedSender<SpeechEvent>) -> MarkerBlock {
    RcBlock::new(move |markers: NonNull<NSArray<AVSpeechSynthesisMarker>>| {
        // SAFETY: AVFAudio passes a live array for the duration of the call.
        let markers = unsafe { markers.as_ref() };
        for marker in markers.iter() {
            // SAFETY: plain property reads on a live marker.
            let (mark, range, offset) =
                unsafe { (marker.mark(), marker.textRange(), marker.byteSampleOffset()) };
            if mark == AVSpeechSynthesisMarkerMark::Word {
                let _ = events.send(SpeechEvent::Word {
                    utf16_index: range.location,
                    byte_offset: offset as u64,
                });
            }
        }
    })
}

/// One buffer as samples; an empty buffer marks the end. `None` for a format we do not read.
fn read_buffer(buffer: &AVAudioBuffer) -> Option<SpeechEvent> {
    // SAFETY: property reads on a live buffer; the buffer list it returns stays valid while the
    // buffer is (for this call), and we only read it.
    let (format, list) = unsafe { (buffer.format(), buffer.audioBufferList().as_ref()) };
    // SAFETY: property reads on a live format.
    let (common, channels, interleaved, rate_hz) = unsafe {
        (
            format.commonFormat(),
            format.channelCount(),
            format.isInterleaved(),
            format.sampleRate(),
        )
    };
    let bytes_per_sample: u32 = match common {
        AVAudioCommonFormat::PCMFormatFloat32 => 4,
        AVAudioCommonFormat::PCMFormatInt16 => 2,
        _ => return None,
    };
    let stride = if interleaved { channels.max(1) } else { 1 };
    let samples = channel_zero(list, common, bytes_per_sample, stride as usize)?;
    if samples.is_empty() {
        return Some(SpeechEvent::Finished);
    }
    Some(SpeechEvent::Audio {
        samples,
        rate_hz,
        bytes_per_frame: bytes_per_sample * stride,
    })
}

/// Channel 0 of the first buffer, as `f32`.
fn channel_zero(
    list: &AudioBufferList,
    common: AVAudioCommonFormat,
    bytes_per_sample: u32,
    stride: usize,
) -> Option<Vec<f32>> {
    if list.mNumberBuffers == 0 {
        return Some(Vec::new());
    }
    let buffer = list.mBuffers[0];
    if buffer.mData.is_null() || buffer.mDataByteSize == 0 {
        return Some(Vec::new());
    }
    let count = buffer.mDataByteSize as usize / bytes_per_sample as usize;
    let samples = if common == AVAudioCommonFormat::PCMFormatFloat32 {
        // SAFETY: `mData` holds `mDataByteSize` bytes of `f32` samples (the format says so),
        // valid and unaliased for this read.
        let data = unsafe { std::slice::from_raw_parts(buffer.mData.cast::<f32>(), count) };
        data.iter().step_by(stride).copied().collect()
    } else {
        // SAFETY: as above, for `i16` samples.
        let data = unsafe { std::slice::from_raw_parts(buffer.mData.cast::<i16>(), count) };
        data.iter()
            .step_by(stride)
            .map(|sample| f32::from(*sample) / 32_768.0)
            .collect()
    };
    Some(samples)
}
