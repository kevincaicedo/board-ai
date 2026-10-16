//! The VoiceProcessingIO audio unit: echo cancellation, noise suppression and gain control
//! around one mic and one speaker (D-006).
//!
//! The unit only initializes at its own client rate (44.1 kHz here; 16, 22.05, 24 and 48 kHz
//! all fail with `kAudioUnitErr_FailedInitialization`), so it runs mono `f32` at that rate and
//! the callbacks convert at the ring boundaries.
//!
//! Each callback owns its state through a raw pointer made from a `Box` at start and freed in
//! `Drop` only after the unit is disposed, when CoreAudio can no longer call it. CoreAudio calls
//! each callback serially, so each state is only ever touched by one call at a time.

use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;
use std::sync::Arc;

use objc2_audio_toolbox::{
    AURenderCallbackStruct, AUVoiceIOOtherAudioDuckingConfiguration,
    AUVoiceIOOtherAudioDuckingLevel, AudioComponentDescription, AudioComponentFindNext,
    AudioComponentInstanceDispose, AudioComponentInstanceNew, AudioOutputUnitStart,
    AudioOutputUnitStop, AudioUnit, AudioUnitGetProperty, AudioUnitInitialize, AudioUnitPropertyID,
    AudioUnitRender, AudioUnitRenderActionFlags, AudioUnitScope, AudioUnitSetProperty,
    AudioUnitUninitialize, kAUVoiceIOProperty_OtherAudioDuckingConfiguration,
    kAUVoiceIOProperty_VoiceProcessingEnableAGC, kAudioOutputUnitProperty_CurrentDevice,
    kAudioOutputUnitProperty_SetInputCallback, kAudioUnitManufacturer_Apple,
    kAudioUnitProperty_Latency, kAudioUnitProperty_SetRenderCallback,
    kAudioUnitProperty_StreamFormat, kAudioUnitScope_Global, kAudioUnitScope_Input,
    kAudioUnitScope_Output, kAudioUnitSubType_VoiceProcessingIO, kAudioUnitType_Output,
};
use objc2_core_audio_types::{
    AudioBuffer, AudioBufferList, AudioStreamBasicDescription, AudioTimeStamp,
    kAudioFormatFlagIsFloat, kAudioFormatFlagIsNonInterleaved, kAudioFormatFlagIsPacked,
    kAudioFormatLinearPCM,
};

use super::OsStatus;
use super::device::{
    built_in_input_device, input_device_named, input_device_names, output_latency_seconds,
};
use crate::audio::resample::Resampler;
use crate::audio::{
    AudioCounters, AudioError, InputChoice, MIC_RATE_HZ, MicWriter, PlaybackRenderer, VOICE_RATE_HZ,
};
use crate::limits::AUDIO_CALLBACK_FRAMES_MAX;

/// The unit's input element (the mic) and output element (the speaker).
const INPUT_ELEMENT: u32 = 1;
const OUTPUT_ELEMENT: u32 = 0;

/// What the unit measured when it started.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct UnitFacts {
    pub unit_rate_hz: u32,
    /// From a frame leaving the voice ring to the speaker, conversion included.
    pub output_latency_seconds: f64,
    /// Whether other apps' audio is ducked as little as macOS allows.
    pub other_audio_ducking_minimized: bool,
}

pub(crate) struct VoiceProcessingUnit {
    unit: AudioUnit,
    capture: *mut CaptureState,
    playback: *mut PlaybackState,
    facts: UnitFacts,
}

// SAFETY: `AudioUnit` is an opaque handle to an object CoreAudio synchronizes internally; it may
// be started, stopped and disposed from any thread. The callback states are reached only by the
// callbacks while the unit lives, and by `Drop` after it is disposed.
unsafe impl Send for VoiceProcessingUnit {}

pub(super) struct CaptureState {
    pub(super) unit: AudioUnit,
    pub(super) rendered: Box<[f32]>,
    pub(super) converted: Box<[f32]>,
    pub(super) resampler: Resampler,
    pub(super) writer: MicWriter,
    pub(super) counters: Arc<AudioCounters>,
}

pub(super) struct PlaybackState {
    pub(super) renderer: PlaybackRenderer,
    pub(super) resampler: Resampler,
    pub(super) counters: Arc<AudioCounters>,
}

impl VoiceProcessingUnit {
    pub(crate) fn start(
        input: &InputChoice,
        writer: MicWriter,
        renderer: PlaybackRenderer,
        counters: Arc<AudioCounters>,
    ) -> Result<Self, AudioError> {
        let unit = new_unit()?;
        let mut owned = OwnedUnit(unit);
        configure(unit, input)?;
        let unit_rate_hz = client_rate_hz(unit)?;
        let other_audio_ducking_minimized = minimize_other_audio_ducking(unit);
        let capture = Box::new(CaptureState {
            unit,
            rendered: vec![0.0; AUDIO_CALLBACK_FRAMES_MAX].into_boxed_slice(),
            converted: vec![0.0; AUDIO_CALLBACK_FRAMES_MAX].into_boxed_slice(),
            resampler: Resampler::new(unit_rate_hz, MIC_RATE_HZ)?,
            writer,
            counters: Arc::clone(&counters),
        });
        let playback = Box::new(PlaybackState {
            renderer,
            resampler: Resampler::new(VOICE_RATE_HZ, unit_rate_hz)?,
            counters,
        });
        let conversion_seconds =
            f64::from(playback.resampler.delay_input_frames()) / f64::from(VOICE_RATE_HZ);
        let capture = Box::into_raw(capture);
        let playback = Box::into_raw(playback);
        // From here `Drop` owns the states, and disposes the unit before freeing them.
        let mut started = Self {
            unit: owned.release(),
            capture,
            playback,
            facts: UnitFacts {
                unit_rate_hz,
                output_latency_seconds: 0.0,
                other_audio_ducking_minimized,
            },
        };
        started.install_callbacks()?;
        check("AudioUnitInitialize", {
            // SAFETY: `unit` is a live instance with formats and callbacks set.
            unsafe { AudioUnitInitialize(unit) }
        })?;
        started.facts.output_latency_seconds = output_latency(unit) + conversion_seconds;
        check("AudioOutputUnitStart", {
            // SAFETY: `unit` is initialized; the callback states outlive it (see `Drop`).
            unsafe { AudioOutputUnitStart(unit) }
        })?;
        Ok(started)
    }

    pub(crate) const fn facts(&self) -> UnitFacts {
        self.facts
    }

    fn install_callbacks(&self) -> Result<(), AudioError> {
        let input = AURenderCallbackStruct {
            inputProc: Some(capture_callback),
            inputProcRefCon: self.capture.cast::<c_void>(),
        };
        set_property(
            self.unit,
            kAudioOutputUnitProperty_SetInputCallback,
            kAudioUnitScope_Global,
            INPUT_ELEMENT,
            &input,
        )?;
        let output = AURenderCallbackStruct {
            inputProc: Some(playback_callback),
            inputProcRefCon: self.playback.cast::<c_void>(),
        };
        set_property(
            self.unit,
            kAudioUnitProperty_SetRenderCallback,
            kAudioUnitScope_Input,
            OUTPUT_ELEMENT,
            &output,
        )
    }
}

impl Drop for VoiceProcessingUnit {
    fn drop(&mut self) {
        // SAFETY: `unit` is live. Stop, uninitialize and dispose in that order; once disposed,
        // CoreAudio never calls the callbacks again, so freeing their states is sound.
        unsafe {
            AudioOutputUnitStop(self.unit);
            AudioUnitUninitialize(self.unit);
            AudioComponentInstanceDispose(self.unit);
        }
        // SAFETY: both pointers came from `Box::into_raw` in `start` and are freed only here.
        unsafe {
            drop(Box::from_raw(self.capture));
            drop(Box::from_raw(self.playback));
        }
    }
}

/// Disposes a unit that failed to start.
struct OwnedUnit(AudioUnit);

impl OwnedUnit {
    fn release(&mut self) -> AudioUnit {
        std::mem::replace(&mut self.0, std::ptr::null_mut())
    }
}

impl Drop for OwnedUnit {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: a live instance that was never started and has no callbacks installed.
            unsafe {
                AudioComponentInstanceDispose(self.0);
            }
        }
    }
}

fn check(call: &'static str, status: OsStatus) -> Result<(), AudioError> {
    if status == 0 {
        Ok(())
    } else {
        Err(AudioError::Os { call, status })
    }
}

fn set_property<T>(
    unit: AudioUnit,
    property: AudioUnitPropertyID,
    scope: AudioUnitScope,
    element: u32,
    value: &T,
) -> Result<(), AudioError> {
    let size = u32::try_from(size_of::<T>()).unwrap_or(u32::MAX);
    // SAFETY: `value` points to a live `T` of `size` bytes, the layout CoreAudio expects for
    // `property` (each caller passes the matching struct).
    let status = unsafe {
        AudioUnitSetProperty(
            unit,
            property,
            scope,
            element,
            std::ptr::from_ref(value).cast::<c_void>(),
            size,
        )
    };
    check("AudioUnitSetProperty", status)
}

fn get_property<T: Copy>(
    unit: AudioUnit,
    property: AudioUnitPropertyID,
    scope: AudioUnitScope,
    element: u32,
    mut value: T,
) -> Result<T, AudioError> {
    let mut size = u32::try_from(size_of::<T>()).unwrap_or(u32::MAX);
    // SAFETY: `value` is a live, writable `T` and `size` tells CoreAudio its exact size.
    let status = unsafe {
        AudioUnitGetProperty(
            unit,
            property,
            scope,
            element,
            NonNull::from(&mut value).cast::<c_void>(),
            NonNull::from(&mut size),
        )
    };
    check("AudioUnitGetProperty", status).map(|()| value)
}

fn new_unit() -> Result<AudioUnit, AudioError> {
    let description = AudioComponentDescription {
        componentType: kAudioUnitType_Output,
        componentSubType: kAudioUnitSubType_VoiceProcessingIO,
        componentManufacturer: kAudioUnitManufacturer_Apple,
        componentFlags: 0,
        componentFlagsMask: 0,
    };
    // SAFETY: `description` is valid for the call; a null start component searches from the
    // beginning.
    let component =
        unsafe { AudioComponentFindNext(std::ptr::null_mut(), NonNull::from(&description)) };
    if component.is_null() {
        return Err(AudioError::VoiceProcessingUnavailable);
    }
    let mut unit: AudioUnit = std::ptr::null_mut();
    // SAFETY: `component` was just found; `unit` receives the new instance.
    let status = unsafe { AudioComponentInstanceNew(component, NonNull::from(&mut unit)) };
    check("AudioComponentInstanceNew", status)?;
    if unit.is_null() {
        return Err(AudioError::VoiceProcessingUnavailable);
    }
    Ok(unit)
}

fn mono_float(rate_hz: f64) -> AudioStreamBasicDescription {
    AudioStreamBasicDescription {
        mSampleRate: rate_hz,
        mFormatID: kAudioFormatLinearPCM,
        mFormatFlags: kAudioFormatFlagIsFloat
            | kAudioFormatFlagIsPacked
            | kAudioFormatFlagIsNonInterleaved,
        mBytesPerPacket: 4,
        mFramesPerPacket: 1,
        mBytesPerFrame: 4,
        mChannelsPerFrame: 1,
        mBitsPerChannel: 32,
        mReserved: 0,
    }
}

/// Picks the mic, turns gain control on, and sets mono `f32` at the unit's own rate on both
/// client sides.
fn configure(unit: AudioUnit, input: &InputChoice) -> Result<(), AudioError> {
    let device = match input {
        InputChoice::SystemDefault => None,
        InputChoice::BuiltIn => Some(built_in_input_device().ok_or(AudioError::NoBuiltInInput)?),
        InputChoice::Named(name) => {
            Some(
                input_device_named(name).ok_or_else(|| AudioError::NoSuchInput {
                    name: name.clone(),
                    available: input_device_names(),
                })?,
            )
        }
    };
    if let Some(device) = device {
        set_property(
            unit,
            kAudioOutputUnitProperty_CurrentDevice,
            kAudioUnitScope_Global,
            INPUT_ELEMENT,
            &device,
        )?;
    }
    let enabled: u32 = 1;
    set_property(
        unit,
        kAUVoiceIOProperty_VoiceProcessingEnableAGC,
        kAudioUnitScope_Global,
        INPUT_ELEMENT,
        &enabled,
    )?;
    let default_format = get_property(
        unit,
        kAudioUnitProperty_StreamFormat,
        kAudioUnitScope_Output,
        INPUT_ELEMENT,
        mono_float(0.0),
    )?;
    let format = mono_float(default_format.mSampleRate);
    set_property(
        unit,
        kAudioUnitProperty_StreamFormat,
        kAudioUnitScope_Output,
        INPUT_ELEMENT,
        &format,
    )?;
    set_property(
        unit,
        kAudioUnitProperty_StreamFormat,
        kAudioUnitScope_Input,
        OUTPUT_ELEMENT,
        &format,
    )
}

fn client_rate_hz(unit: AudioUnit) -> Result<u32, AudioError> {
    let format = get_property(
        unit,
        kAudioUnitProperty_StreamFormat,
        kAudioUnitScope_Output,
        INPUT_ELEMENT,
        mono_float(0.0),
    )?;
    let rate = format.mSampleRate.round();
    if rate >= 1.0 && rate <= f64::from(u32::MAX) {
        Ok(rate as u32)
    } else {
        Err(AudioError::UnsupportedRate { rate_hz: 0 })
    }
}

/// macOS 14+ ducks other apps' audio while voice processing runs; ask for the least ducking.
/// Best effort: a refusal is reported in the facts, never fatal.
fn minimize_other_audio_ducking(unit: AudioUnit) -> bool {
    let configuration = AUVoiceIOOtherAudioDuckingConfiguration {
        mEnableAdvancedDucking: 0,
        mDuckingLevel: AUVoiceIOOtherAudioDuckingLevel::Min,
    };
    set_property(
        unit,
        kAUVoiceIOProperty_OtherAudioDuckingConfiguration,
        kAudioUnitScope_Global,
        0,
        &configuration,
    )
    .is_ok()
}

/// The output device's latency plus the unit's own processing latency.
fn output_latency(unit: AudioUnit) -> f64 {
    let device = get_property(
        unit,
        kAudioOutputUnitProperty_CurrentDevice,
        kAudioUnitScope_Global,
        OUTPUT_ELEMENT,
        0_u32,
    );
    let device_seconds = device.map(output_latency_seconds).unwrap_or(0.0);
    let unit_seconds = get_property(
        unit,
        kAudioUnitProperty_Latency,
        kAudioUnitScope_Global,
        0,
        0.0_f64,
    )
    .unwrap_or(0.0);
    device_seconds + unit_seconds.max(0.0)
}

/// The mic callback: pull the processed input from the unit, convert it to 16 kHz, and write
/// it to the mic ring. Real-time: no allocation, lock, log or blocking call.
unsafe extern "C-unwind" fn capture_callback(
    context: NonNull<c_void>,
    flags: NonNull<AudioUnitRenderActionFlags>,
    timestamp: NonNull<AudioTimeStamp>,
    bus: u32,
    frames: u32,
    _data: *mut AudioBufferList,
) -> OsStatus {
    // SAFETY: `context` is the `CaptureState` installed in `start`, alive until the unit is
    // disposed, and only this callback (called serially) touches it.
    let state = unsafe { context.cast::<CaptureState>().as_mut() };
    let frames = frames as usize;
    if frames > state.rendered.len() {
        state.counters.oversized_cycles.increment();
        return 0;
    }
    let mut list = AudioBufferList {
        mNumberBuffers: 1,
        mBuffers: [AudioBuffer {
            mNumberChannels: 1,
            mDataByteSize: u32::try_from(frames * 4).unwrap_or(u32::MAX),
            mData: state.rendered.as_mut_ptr().cast::<c_void>(),
        }],
    };
    // SAFETY: the unit is live; `list` points to `frames` writable floats in `rendered`, the
    // mono non-interleaved format set on this element.
    let status = unsafe {
        AudioUnitRender(
            state.unit,
            flags.as_ptr(),
            timestamp,
            bus,
            u32::try_from(frames).unwrap_or(u32::MAX),
            NonNull::from(&mut list),
        )
    };
    if status != 0 {
        state.counters.capture_errors.increment();
        return status;
    }
    let outcome = catch_unwind(AssertUnwindSafe(|| convert_capture(state, frames)));
    if outcome.is_err() {
        state.counters.callback_panics.increment();
    }
    0
}

/// One capture cycle after `AudioUnitRender`: convert to 16 kHz and write the mic ring.
pub(super) fn convert_capture(state: &mut CaptureState, frames: usize) {
    let CaptureState {
        rendered,
        converted,
        resampler,
        writer,
        ..
    } = state;
    let mut count = 0;
    resampler.push(&rendered[..frames], |sample| {
        if let Some(slot) = converted.get_mut(count) {
            *slot = sample;
            count += 1;
        }
    });
    writer.write(&converted[..count]);
}

/// The speaker callback: pull 24 kHz voice through the renderer and convert it to the unit's
/// rate. Real-time: no allocation, lock, log or blocking call.
unsafe extern "C-unwind" fn playback_callback(
    context: NonNull<c_void>,
    _flags: NonNull<AudioUnitRenderActionFlags>,
    _timestamp: NonNull<AudioTimeStamp>,
    _bus: u32,
    frames: u32,
    data: *mut AudioBufferList,
) -> OsStatus {
    // SAFETY: `context` is the `PlaybackState` installed in `start`, alive until the unit is
    // disposed, and only this callback (called serially) touches it.
    let state = unsafe { context.cast::<PlaybackState>().as_mut() };
    // SAFETY: CoreAudio passes a buffer list for the mono non-interleaved format set on this
    // element: one buffer of `frames` floats that it owns for the duration of the call.
    let Some(out) = (unsafe { output_slice(data, frames) }) else {
        state.counters.oversized_cycles.increment();
        return 0;
    };
    let outcome = catch_unwind(AssertUnwindSafe(|| render_playback(state, out)));
    if outcome.is_err() {
        state.counters.callback_panics.increment();
    }
    0
}

/// One playback cycle: the voice at 24 kHz through the renderer, converted to the unit's rate.
pub(super) fn render_playback(state: &mut PlaybackState, out: &mut [f32]) {
    let PlaybackState {
        renderer,
        resampler,
        ..
    } = state;
    renderer.begin_cycle();
    resampler.pull(out, || renderer.next_frame());
    renderer.end_cycle();
}

/// The first buffer of `data` as `frames` floats, or `None` when it is missing or too small.
///
/// # Safety
///
/// `data` must be null or point to a buffer list whose first buffer holds `mDataByteSize`
/// writable bytes for the duration of the returned borrow.
unsafe fn output_slice<'a>(data: *mut AudioBufferList, frames: u32) -> Option<&'a mut [f32]> {
    // SAFETY: the caller guarantees `data` is null or a valid buffer list.
    let list = unsafe { data.as_mut() }?;
    if list.mNumberBuffers == 0 {
        return None;
    }
    let buffer = list.mBuffers[0];
    let frames = frames as usize;
    if buffer.mData.is_null() || (buffer.mDataByteSize as usize) < frames * 4 {
        return None;
    }
    // SAFETY: `mData` holds at least `frames` floats (checked above), owned by CoreAudio for
    // the callback, and nothing else references it during the call.
    Some(unsafe { std::slice::from_raw_parts_mut(buffer.mData.cast::<f32>(), frames) })
}
