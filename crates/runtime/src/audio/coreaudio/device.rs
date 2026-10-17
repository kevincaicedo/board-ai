//! Audio device queries through the HAL (`AudioObjectGetPropertyData`): the output latency
//! that turns the played-frames counter into what is heard, and a snapshot of the devices and
//! the system defaults, from which the supervisor plans the unit's route.

use std::ffi::c_void;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2_foundation::NSString;

use objc2_core_audio::{
    AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize, AudioObjectID,
    AudioObjectPropertyAddress, AudioObjectPropertyScope, AudioObjectPropertySelector,
    kAudioDevicePropertyBufferFrameSize, kAudioDevicePropertyLatency,
    kAudioDevicePropertyNominalSampleRate, kAudioDevicePropertySafetyOffset,
    kAudioDevicePropertyStreams, kAudioDevicePropertyTransportType,
    kAudioDeviceTransportTypeAggregate, kAudioDeviceTransportTypeBuiltIn,
    kAudioDeviceTransportTypeVirtual, kAudioHardwarePropertyDefaultInputDevice,
    kAudioHardwarePropertyDefaultOutputDevice, kAudioHardwarePropertyDevices,
    kAudioObjectPropertyElementMain, kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal,
    kAudioObjectPropertyScopeInput, kAudioObjectPropertyScopeOutput, kAudioObjectSystemObject,
    kAudioStreamPropertyLatency,
};

use super::OsStatus;
use crate::audio::devices::{Device, DeviceId, DeviceSnapshot};
use crate::limits::AUDIO_DEVICES_MAX;

pub(super) fn address(
    selector: AudioObjectPropertySelector,
    scope: AudioObjectPropertyScope,
) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain,
    }
}

/// Reads a fixed-size property into `value`.
fn read<T: Copy>(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
    scope: AudioObjectPropertyScope,
    mut value: T,
) -> Result<T, OsStatus> {
    let address = address(selector, scope);
    let mut size = u32::try_from(size_of::<T>()).unwrap_or(u32::MAX);
    // SAFETY: `address` and `size` are valid for the call; `value` is a live, writable `T` and
    // `size` tells the HAL it holds exactly `size_of::<T>()` bytes, which it never exceeds.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&address),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut value).cast::<c_void>(),
        )
    };
    if status == 0 { Ok(value) } else { Err(status) }
}

/// Reads an array property of `u32` ids, up to [`AUDIO_DEVICES_MAX`] of them.
fn read_ids(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
    scope: AudioObjectPropertyScope,
) -> Result<Vec<AudioObjectID>, OsStatus> {
    let address = address(selector, scope);
    let mut size: u32 = 0;
    // SAFETY: `address` and `size` are valid for the call; no qualifier is passed.
    let status = unsafe {
        AudioObjectGetPropertyDataSize(
            object,
            NonNull::from(&address),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
        )
    };
    if status != 0 {
        return Err(status);
    }
    let count = (size as usize / size_of::<AudioObjectID>()).min(AUDIO_DEVICES_MAX);
    if count == 0 {
        return Ok(Vec::new());
    }
    let mut ids: Vec<AudioObjectID> = vec![0; count];
    let mut size = u32::try_from(count * size_of::<AudioObjectID>()).unwrap_or(u32::MAX);
    // SAFETY: `ids` holds `count` ids and `size` says exactly that many bytes, so the HAL
    // writes within the vector.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&address),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::new(ids.as_mut_ptr())
                .unwrap_or(NonNull::dangling())
                .cast::<c_void>(),
        )
    };
    if status != 0 {
        return Err(status);
    }
    ids.truncate(size as usize / size_of::<AudioObjectID>());
    Ok(ids)
}

/// Seconds between a frame leaving the render callback and reaching the speaker: the device's
/// latency, its safety offset, one IO buffer, and the output stream's own latency. Parts the
/// device does not report count as zero.
pub(crate) fn output_latency_seconds(device: AudioObjectID) -> f64 {
    let scope = kAudioObjectPropertyScopeOutput;
    let rate_hz = read(
        device,
        kAudioDevicePropertyNominalSampleRate,
        scope,
        0.0_f64,
    )
    .ok()
    .filter(|rate| *rate > 0.0);
    let Some(rate_hz) = rate_hz else {
        return 0.0;
    };
    let frames = |selector| read(device, selector, scope, 0_u32).unwrap_or(0);
    let stream_frames = read_ids(device, kAudioDevicePropertyStreams, scope)
        .ok()
        .and_then(|streams| streams.first().copied())
        .and_then(|stream| {
            read(
                stream,
                kAudioStreamPropertyLatency,
                kAudioObjectPropertyScopeGlobal,
                0_u32,
            )
            .ok()
        })
        .unwrap_or(0);
    let total = frames(kAudioDevicePropertyLatency)
        + frames(kAudioDevicePropertySafetyOffset)
        + frames(kAudioDevicePropertyBufferFrameSize)
        + stream_frames;
    f64::from(total) / rate_hz
}

/// Every device with its name and directions, and the system's default input and output.
/// A device whose name cannot be read is left out (nothing could choose it).
pub(crate) fn snapshot() -> DeviceSnapshot {
    let Ok(system) = AudioObjectID::try_from(kAudioObjectSystemObject) else {
        return DeviceSnapshot::default();
    };
    let ids = read_ids(
        system,
        kAudioHardwarePropertyDevices,
        kAudioObjectPropertyScopeGlobal,
    )
    .unwrap_or_default();
    let has_streams = |device, scope| {
        read_ids(device, kAudioDevicePropertyStreams, scope)
            .is_ok_and(|streams| !streams.is_empty())
    };
    let devices = ids
        .into_iter()
        .filter_map(|id| {
            let transport = read(
                id,
                kAudioDevicePropertyTransportType,
                kAudioObjectPropertyScopeGlobal,
                0_u32,
            );
            let virtual_device = transport.is_ok_and(|transport| {
                transport == kAudioDeviceTransportTypeVirtual
                    || transport == kAudioDeviceTransportTypeAggregate
            });
            Some(Device {
                id: DeviceId(id),
                name: device_name(id)?,
                has_input: has_streams(id, kAudioObjectPropertyScopeInput),
                has_output: has_streams(id, kAudioObjectPropertyScopeOutput),
                built_in: transport == Ok(kAudioDeviceTransportTypeBuiltIn),
                virtual_device,
            })
        })
        .collect();
    // `kAudioObjectUnknown` (0) means there is no default.
    let default = |selector| {
        read(system, selector, kAudioObjectPropertyScopeGlobal, 0_u32)
            .ok()
            .filter(|id| *id != 0)
            .map(DeviceId)
    };
    DeviceSnapshot {
        devices,
        default_input: default(kAudioHardwarePropertyDefaultInputDevice),
        default_output: default(kAudioHardwarePropertyDefaultOutputDevice),
    }
}

/// The names of the devices that can record, for settings and error messages.
pub(crate) fn input_device_names() -> Vec<String> {
    snapshot()
        .devices
        .into_iter()
        .filter(|device| device.has_input)
        .map(|device| device.name)
        .collect()
}

fn device_name(device: AudioObjectID) -> Option<String> {
    let name = read(
        device,
        kAudioObjectPropertyName,
        kAudioObjectPropertyScopeGlobal,
        std::ptr::null_mut::<NSString>(),
    )
    .ok()?;
    // SAFETY: the HAL returns the name as a `CFStringRef` the caller owns (one retain), and
    // CFString is toll-free bridged to NSString, so `Retained` releases it exactly once. A null
    // result is `None`.
    let name = unsafe { Retained::from_raw(name) }?;
    Some(name.to_string())
}
