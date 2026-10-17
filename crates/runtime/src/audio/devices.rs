//! Which devices the audio unit should run on, decided from a snapshot of what macOS has
//! (LLD §3.4, "Devices"). Pure: the CoreAudio side reads the snapshot, the supervisor acts on
//! the plan, and tests feed both with fakes.
//!
//! The rules:
//!
//! - A system-default choice follows macOS's default as it changes.
//! - A named device (or the built-in microphone) is used while it is there. When it is not, the
//!   system default stands in and the user is told; when it comes back it is used again.
//! - A stand-in is a real device: when the default is virtual (on the owner's Mac the default
//!   input is a mixer app's loopback, which hears nothing) the built-in one stands in instead.

use std::fmt;

use super::{AudioError, InputChoice};
use crate::limits::AUDIO_DEVICE_NAME_MAX_BYTES;

/// A CoreAudio device id (`AudioObjectID`). Ids are not reused while macOS runs, but a device
/// plugged in again may get a new one, so names are what choices match.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DeviceId(pub u32);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub id: DeviceId,
    pub name: String,
    pub has_input: bool,
    pub has_output: bool,
    pub built_in: bool,
    /// A virtual or aggregate device (a mixer app's loopback, a meeting app's device): often
    /// silent as a microphone, so never the one a fallback picks when a real one exists.
    pub virtual_device: bool,
}

/// What macOS has right now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceSnapshot {
    pub devices: Vec<Device>,
    pub default_input: Option<DeviceId>,
    pub default_output: Option<DeviceId>,
}

impl DeviceSnapshot {
    fn device(&self, id: Option<DeviceId>) -> Option<&Device> {
        let id = id?;
        self.devices.iter().find(|device| device.id == id)
    }

    fn named(&self, name: &str, input: bool) -> Option<&Device> {
        self.devices.iter().find(|device| {
            (if input {
                device.has_input
            } else {
                device.has_output
            }) && device.name.eq_ignore_ascii_case(name.trim())
        })
    }
}

/// Which speaker the unit plays on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum OutputChoice {
    /// The system's default output, followed as it changes (headphones plugged in or out).
    #[default]
    SystemDefault,
    /// The output device with this name, as macOS shows it (ignoring case).
    Named(String),
}

const OUTPUT_SYSTEM_DEFAULT: &str = "system_default";

impl OutputChoice {
    /// `system_default`, or a device name.
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value.trim() {
            OUTPUT_SYSTEM_DEFAULT => Self::SystemDefault,
            name => Self::Named(name.to_owned()),
        }
    }

    #[must_use]
    pub fn as_setting(&self) -> &str {
        match self {
            Self::SystemDefault => OUTPUT_SYSTEM_DEFAULT,
            Self::Named(name) => name,
        }
    }
}

/// One side of a route: the device and whether the unit must be told to use it. A system
/// default is not pinned, so the unit opens it the way it always has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub device: DeviceId,
    pub name: String,
    pub pinned: bool,
}

/// The devices one audio unit runs on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    pub input: Endpoint,
    pub output: Endpoint,
}

impl Route {
    /// The stand-ins on both sides, for when a chosen device is there but will not start.
    pub(crate) fn stand_ins(snapshot: &DeviceSnapshot) -> Result<Self, AudioError> {
        Ok(Self {
            input: stand_in(snapshot, true)?,
            output: stand_in(snapshot, false)?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceKind {
    Input,
    Output,
}

/// Why a chosen device is not the one in use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Absence {
    /// It was there a moment ago.
    Unplugged,
    /// It was not there when audio started.
    NotConnected,
    /// It is there, but no audio unit would start on it.
    WontStart,
}

/// What changed about the devices, for the user and the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceEvent {
    /// The chosen device is not in use; the system default stands in.
    Missing {
        kind: DeviceKind,
        wanted: String,
        using: String,
        why: Absence,
    },
    /// The chosen device is back and in use again.
    Returned { kind: DeviceKind, name: String },
    /// The unit was rebuilt and now runs on these devices.
    Switched { input: String, output: String },
    /// No unit could start; there is no sound or microphone until the devices change again.
    Lost { error: AudioError },
}

/// The route for the choices, and which chosen devices are missing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Plan {
    pub(crate) route: Route,
    pub(crate) missing_input: Option<String>,
    pub(crate) missing_output: Option<String>,
}

/// The route the choices ask for in `snapshot`. Fails only when there is no default to fall
/// back on.
pub(crate) fn plan(
    input: &InputChoice,
    output: &OutputChoice,
    snapshot: &DeviceSnapshot,
) -> Result<Plan, AudioError> {
    let (input, missing_input) = match input {
        InputChoice::SystemDefault => (default_endpoint(snapshot, true)?, None),
        InputChoice::BuiltIn => {
            let built_in = snapshot
                .devices
                .iter()
                .find(|device| device.has_input && device.built_in);
            match built_in {
                Some(device) => (pinned(device), None),
                None => (
                    stand_in(snapshot, true)?,
                    Some("the built-in microphone".to_owned()),
                ),
            }
        }
        InputChoice::Named(name) => match snapshot.named(name, true) {
            Some(device) => (pinned(device), None),
            None => (stand_in(snapshot, true)?, Some(bounded(name))),
        },
    };
    let (output, missing_output) = match output {
        OutputChoice::SystemDefault => (default_endpoint(snapshot, false)?, None),
        OutputChoice::Named(name) => match snapshot.named(name, false) {
            Some(device) => (pinned(device), None),
            None => (stand_in(snapshot, false)?, Some(bounded(name))),
        },
    };
    Ok(Plan {
        route: Route { input, output },
        missing_input,
        missing_output,
    })
}

/// What the user hears about from one plan to the next: a chosen device that went missing or
/// came back. `previous` is `None` for the first plan, when a missing device was never there.
pub(crate) fn changes(previous: Option<&Plan>, next: &Plan) -> Vec<DeviceEvent> {
    let sides = [
        (
            DeviceKind::Input,
            previous.map(|plan| &plan.missing_input),
            &next.missing_input,
            &next.route.input,
        ),
        (
            DeviceKind::Output,
            previous.map(|plan| &plan.missing_output),
            &next.missing_output,
            &next.route.output,
        ),
    ];
    sides
        .into_iter()
        .filter_map(|(kind, before, now, using)| match (before, now) {
            (None, Some(wanted)) => Some(DeviceEvent::Missing {
                kind,
                wanted: wanted.clone(),
                using: using.name.clone(),
                why: Absence::NotConnected,
            }),
            (Some(None), Some(wanted)) => Some(DeviceEvent::Missing {
                kind,
                wanted: wanted.clone(),
                using: using.name.clone(),
                why: Absence::Unplugged,
            }),
            (Some(Some(_)), None) => Some(DeviceEvent::Returned {
                kind,
                name: using.name.clone(),
            }),
            (Some(Some(_)), Some(_)) | (Some(None) | None, None) => None,
        })
        .collect()
}

/// The pinned devices of `planned` that `started` does not use, because they would not start.
pub(crate) fn refused(planned: &Route, started: &Route) -> Vec<DeviceEvent> {
    [
        (DeviceKind::Input, &planned.input, &started.input),
        (DeviceKind::Output, &planned.output, &started.output),
    ]
    .into_iter()
    .filter(|(_, planned, started)| planned.pinned && planned.device != started.device)
    .map(|(kind, planned, started)| DeviceEvent::Missing {
        kind,
        wanted: planned.name.clone(),
        using: started.name.clone(),
        why: Absence::WontStart,
    })
    .collect()
}

/// What stands in for a missing choice: the system default when it is a real device, else the
/// built-in one (pinned, since it is not the default), else the default anyway.
fn stand_in(snapshot: &DeviceSnapshot, input: bool) -> Result<Endpoint, AudioError> {
    let default = default_endpoint(snapshot, input)?;
    let default_is_virtual = snapshot
        .device(Some(default.device))
        .is_some_and(|device| device.virtual_device);
    if !default_is_virtual {
        return Ok(default);
    }
    let built_in = snapshot.devices.iter().find(|device| {
        device.built_in
            && !device.virtual_device
            && (if input {
                device.has_input
            } else {
                device.has_output
            })
    });
    Ok(built_in.map_or(default, pinned))
}

fn default_endpoint(snapshot: &DeviceSnapshot, input: bool) -> Result<Endpoint, AudioError> {
    let id = if input {
        snapshot.default_input
    } else {
        snapshot.default_output
    };
    let device = snapshot.device(id).ok_or(if input {
        AudioError::NoInputDevice
    } else {
        AudioError::NoOutputDevice
    })?;
    Ok(Endpoint {
        device: device.id,
        name: bounded(&device.name),
        pinned: false,
    })
}

fn pinned(device: &Device) -> Endpoint {
    Endpoint {
        device: device.id,
        name: bounded(&device.name),
        pinned: true,
    }
}

/// A device name short enough for a caption and a log line.
fn bounded(name: &str) -> String {
    let mut end = name.len().min(AUDIO_DEVICE_NAME_MAX_BYTES);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name[..end].to_owned()
}

impl fmt::Display for DeviceKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Input => "microphone",
            Self::Output => "speaker",
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::{
        Absence, Device, DeviceEvent, DeviceId, DeviceKind, DeviceSnapshot, OutputChoice, changes,
        plan,
    };
    use crate::audio::{AudioError, InputChoice};

    pub(crate) const BUILT_IN_MIC: u32 = 10;
    pub(crate) const SPEAKERS: u32 = 11;
    pub(crate) const SHURE: u32 = 20;
    pub(crate) const HEADPHONES: u32 = 30;
    /// A mixer app's virtual device, which hears nothing (the owner's default input).
    pub(crate) const MIXER: u32 = 40;

    fn device(id: u32, name: &str, has_input: bool, has_output: bool) -> Device {
        Device {
            id: DeviceId(id),
            name: name.to_owned(),
            has_input,
            has_output,
            built_in: id < 20,
            virtual_device: id == MIXER,
        }
    }

    /// A MacBook with its own mic and speakers, plus whatever `extra` is plugged in.
    pub(crate) fn mac(extra: &[u32], default_input: u32, default_output: u32) -> DeviceSnapshot {
        let mut devices = vec![
            device(BUILT_IN_MIC, "MacBook Pro Microphone", true, false),
            device(SPEAKERS, "MacBook Pro Speakers", false, true),
        ];
        for id in extra {
            devices.push(match *id {
                SHURE => device(SHURE, "Shure MV7+", true, true),
                MIXER => device(MIXER, "MOTIV Mix Virtual", true, true),
                _ => device(HEADPHONES, "AirPods Pro", true, true),
            });
        }
        DeviceSnapshot {
            devices,
            default_input: Some(DeviceId(default_input)),
            default_output: Some(DeviceId(default_output)),
        }
    }

    fn shure() -> InputChoice {
        InputChoice::Named("shure mv7+".to_owned())
    }

    #[test]
    fn defaults_follow_macos_and_named_devices_are_pinned() {
        let snapshot = mac(&[SHURE], SHURE, SPEAKERS);
        let followed = plan(
            &InputChoice::SystemDefault,
            &OutputChoice::SystemDefault,
            &snapshot,
        )
        .expect("plan");
        assert_eq!(followed.route.input.device, DeviceId(SHURE));
        assert!(!followed.route.input.pinned && !followed.route.output.pinned);
        let named = plan(&shure(), &OutputChoice::SystemDefault, &snapshot).expect("plan");
        assert_eq!(named.route.input.name, "Shure MV7+");
        assert!(named.route.input.pinned);
        assert_eq!(named.missing_input, None);
        let built_in = plan(
            &InputChoice::BuiltIn,
            &OutputChoice::SystemDefault,
            &snapshot,
        );
        assert_eq!(
            built_in.expect("plan").route.input.device,
            DeviceId(BUILT_IN_MIC)
        );
    }

    #[test]
    fn a_missing_choice_falls_back_to_the_default_and_says_so() {
        let unplugged = mac(&[], BUILT_IN_MIC, SPEAKERS);
        let output = OutputChoice::Named("AirPods Pro".to_owned());
        let fallen = plan(&shure(), &output, &unplugged).expect("plan");
        assert_eq!(fallen.route.input.device, DeviceId(BUILT_IN_MIC));
        assert!(!fallen.route.input.pinned);
        assert_eq!(fallen.missing_input.as_deref(), Some("shure mv7+"));
        assert_eq!(fallen.missing_output.as_deref(), Some("AirPods Pro"));
        let none = DeviceSnapshot::default();
        assert_eq!(
            plan(
                &InputChoice::SystemDefault,
                &OutputChoice::SystemDefault,
                &none
            ),
            Err(AudioError::NoInputDevice)
        );
    }

    #[test]
    fn a_virtual_default_never_stands_in_for_a_missing_microphone() {
        // The owner's Mac: the default input is a mixer's loopback that hears nothing.
        let unplugged = mac(&[MIXER], MIXER, SPEAKERS);
        let fallen = plan(&shure(), &OutputChoice::SystemDefault, &unplugged).expect("plan");
        assert_eq!(fallen.route.input.name, "MacBook Pro Microphone");
        assert!(
            fallen.route.input.pinned,
            "the built-in one is not the default"
        );
        // Chosen on purpose, the default is followed even when it is virtual.
        let followed = plan(
            &InputChoice::SystemDefault,
            &OutputChoice::SystemDefault,
            &unplugged,
        )
        .expect("plan");
        assert_eq!(followed.route.input.device, DeviceId(MIXER));
    }

    #[test]
    fn going_missing_and_coming_back_are_told_once_each() {
        let output = OutputChoice::SystemDefault;
        let plugged = plan(&shure(), &output, &mac(&[SHURE], BUILT_IN_MIC, SPEAKERS));
        let plugged = plugged.expect("plan");
        let unplugged = plan(&shure(), &output, &mac(&[], BUILT_IN_MIC, SPEAKERS));
        let unplugged = unplugged.expect("plan");
        assert_eq!(changes(None, &plugged), []);
        assert_eq!(
            changes(Some(&plugged), &unplugged),
            [DeviceEvent::Missing {
                kind: DeviceKind::Input,
                wanted: "shure mv7+".to_owned(),
                using: "MacBook Pro Microphone".to_owned(),
                why: Absence::Unplugged,
            }]
        );
        assert_eq!(changes(Some(&unplugged), &unplugged), []);
        assert_eq!(
            changes(Some(&unplugged), &plugged),
            [DeviceEvent::Returned {
                kind: DeviceKind::Input,
                name: "Shure MV7+".to_owned(),
            }]
        );
        let at_start = changes(None, &unplugged);
        assert!(matches!(
            at_start.as_slice(),
            [DeviceEvent::Missing {
                why: Absence::NotConnected,
                ..
            }]
        ));
    }
}
