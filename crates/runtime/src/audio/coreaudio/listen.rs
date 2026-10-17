//! CoreAudio property listeners on the system object: the device list, the default input and
//! the default output. Each change only signals the device supervisor; it reads a fresh
//! snapshot itself, after the burst of notifications one plug causes has settled.
//!
//! CoreAudio calls listeners on its own notification thread, not the real-time one. The
//! listener still does no more than a non-blocking send on a bounded channel.

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::mpsc::SyncSender;

use objc2_core_audio::{
    AudioObjectAddPropertyListener, AudioObjectID, AudioObjectPropertyAddress,
    AudioObjectRemovePropertyListener, kAudioHardwarePropertyDefaultInputDevice,
    kAudioHardwarePropertyDefaultOutputDevice, kAudioHardwarePropertyDevices,
    kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject,
};

use super::OsStatus;
use super::device::address;
use crate::audio::AudioError;
use crate::audio::supervisor::Signal;

const SELECTORS: [u32; 3] = [
    kAudioHardwarePropertyDevices,
    kAudioHardwarePropertyDefaultInputDevice,
    kAudioHardwarePropertyDefaultOutputDevice,
];

/// What the listener reaches through its client data.
struct ListenerTarget {
    signals: SyncSender<Signal>,
}

/// The registered listeners; dropping this removes them.
pub(crate) struct DeviceListeners {
    target: *mut ListenerTarget,
    registered_count: usize,
}

// SAFETY: `target` points to a `ListenerTarget` that is never freed (see `Drop`) and holds only
// a `SyncSender`, which is `Send` and `Sync`; the pointer itself is only passed to CoreAudio's
// add and remove calls, which may be made from any thread.
unsafe impl Send for DeviceListeners {}

impl DeviceListeners {
    /// Registers the three listeners; each change sends `Signal::DevicesChanged` on `signals`
    /// (dropped when the channel is full: the signal already waiting covers it).
    pub(crate) fn register(signals: SyncSender<Signal>) -> Result<Self, AudioError> {
        let system = system_object()?;
        let target = Box::into_raw(Box::new(ListenerTarget { signals }));
        let mut listeners = Self {
            target,
            registered_count: 0,
        };
        for selector in SELECTORS {
            let address = address(selector, kAudioObjectPropertyScopeGlobal);
            // SAFETY: `address` is valid for the call; `devices_changed` matches
            // `AudioObjectPropertyListenerProc`; the client data outlives every call (it is
            // never freed).
            let status = unsafe {
                AudioObjectAddPropertyListener(
                    system,
                    NonNull::from(&address),
                    Some(devices_changed),
                    listeners.target.cast::<c_void>(),
                )
            };
            if status != 0 {
                return Err(AudioError::Os {
                    call: "AudioObjectAddPropertyListener",
                    status,
                });
            }
            listeners.registered_count += 1;
        }
        Ok(listeners)
    }
}

impl Drop for DeviceListeners {
    fn drop(&mut self) {
        let Ok(system) = system_object() else {
            return;
        };
        for selector in SELECTORS.iter().take(self.registered_count) {
            let address = address(*selector, kAudioObjectPropertyScopeGlobal);
            // SAFETY: the same object, address, proc and client data as the registration.
            unsafe {
                AudioObjectRemovePropertyListener(
                    system,
                    NonNull::from(&address),
                    Some(devices_changed),
                    self.target.cast::<c_void>(),
                );
            }
        }
        // `target` is leaked on purpose: CoreAudio documents no point after which a listener it
        // was calling on its notification thread has returned, so its data must outlive the
        // process. It is one sender (a few dozen bytes) per audio start.
    }
}

fn system_object() -> Result<AudioObjectID, AudioError> {
    AudioObjectID::try_from(kAudioObjectSystemObject).map_err(|_| AudioError::Os {
        call: "kAudioObjectSystemObject",
        status: 0,
    })
}

/// The listener: a non-blocking send, nothing else.
unsafe extern "C-unwind" fn devices_changed(
    _object: AudioObjectID,
    _count: u32,
    _addresses: NonNull<AudioObjectPropertyAddress>,
    client: *mut c_void,
) -> OsStatus {
    // SAFETY: `client` is the `ListenerTarget` given at registration, never freed, and only
    // read here (`SyncSender::try_send` takes `&self`).
    if let Some(target) = unsafe { client.cast::<ListenerTarget>().as_ref() } {
        let _ = target.signals.try_send(Signal::DevicesChanged);
    }
    0
}
