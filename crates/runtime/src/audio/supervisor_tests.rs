//! The device supervisor against a fake device world: devices come and go, defaults change,
//! units refuse to start or stop rendering. The real rings and renderer move between fake
//! units, so playback continuity is checked on the real counter. No hardware.

use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::devices::tests::{BUILT_IN_MIC, HEADPHONES, MIXER, SHURE, SPEAKERS, mac};
use super::devices::{Absence, DeviceEvent, DeviceId, DeviceKind, DeviceSnapshot, Route};
use super::supervisor::{
    AudioBackend, SharedFacts, Signal, Supervisor, SupervisorHandle, Timing, UnitFacts, UnitParts,
};
use super::{AudioError, InputChoice, OutputChoice, VoiceOutput, mic_ring, voice_ring};
use crate::limits::AUDIO_REBUILD_ATTEMPTS_MAX;

#[derive(Default)]
struct World {
    snapshot: DeviceSnapshot,
    /// Starts refused before one succeeds.
    refusals: u32,
    refuse_all: bool,
    starts: Vec<Route>,
    stops: usize,
    /// The running unit's ring ends, which the test renders through as its device.
    active: Option<UnitParts>,
    latency_seconds: f64,
}

struct FakeBackend(Arc<Mutex<World>>);

struct FakeUnit;

impl AudioBackend for FakeBackend {
    type Unit = FakeUnit;

    fn snapshot(&mut self) -> DeviceSnapshot {
        self.0.lock().expect("lock").snapshot.clone()
    }

    fn start(
        &mut self,
        route: &Route,
        parts: UnitParts,
    ) -> Result<(FakeUnit, UnitFacts), (AudioError, UnitParts)> {
        let mut world = self.0.lock().expect("lock");
        world.starts.push(route.clone());
        if world.refuse_all || world.refusals > 0 {
            world.refusals = world.refusals.saturating_sub(1);
            let error = AudioError::Os {
                call: "AudioOutputUnitStart",
                status: -1,
            };
            return Err((error, parts));
        }
        world.active = Some(parts);
        let facts = UnitFacts {
            unit_rate_hz: 44_100,
            output_latency_seconds: world.latency_seconds,
            other_audio_ducking_minimized: true,
        };
        Ok((FakeUnit, facts))
    }

    fn stop(&mut self, _unit: FakeUnit) -> UnitParts {
        let mut world = self.0.lock().expect("lock");
        world.stops += 1;
        world.active.take().expect("a running unit holds the parts")
    }
}

struct Rig {
    world: Arc<Mutex<World>>,
    signals: SyncSender<Signal>,
    events: tokio::sync::mpsc::Receiver<DeviceEvent>,
    output: VoiceOutput,
    handle: Option<SupervisorHandle>,
}

/// Fast enough for tests; no stall unless a test asks for one.
fn quick() -> Timing {
    Timing {
        settle: Duration::from_millis(5),
        settle_max: Duration::from_millis(50),
        watchdog: Duration::from_millis(5),
        stall: Duration::from_secs(3_600),
        retry: Duration::from_millis(1),
    }
}

fn shure() -> InputChoice {
    InputChoice::Named("Shure MV7+".to_owned())
}

fn rig(
    snapshot: DeviceSnapshot,
    input: InputChoice,
    timing: Timing,
    prepare: impl FnOnce(&mut World),
) -> Result<Rig, AudioError> {
    let mut world = World {
        snapshot,
        latency_seconds: 0.03,
        ..World::default()
    };
    prepare(&mut world);
    let world = Arc::new(Mutex::new(world));
    let (mic, _reader) = mic_ring(1_024);
    let (output, renderer) = voice_ring(24_000, 0);
    let (events_sender, events) = tokio::sync::mpsc::channel(64);
    let supervisor = Supervisor::start(
        FakeBackend(Arc::clone(&world)),
        (input, OutputChoice::SystemDefault),
        UnitParts { mic, renderer },
        events_sender,
        SharedFacts::default(),
        timing,
    )?;
    let (signals, receiver) = std::sync::mpsc::sync_channel(4);
    let handle = SupervisorHandle::spawn(supervisor, signals.clone(), receiver)?;
    Ok(Rig {
        world,
        signals,
        events,
        output,
        handle: Some(handle),
    })
}

impl Rig {
    /// Changes the world, then tells the supervisor, as CoreAudio's listeners would.
    fn change(&self, change: impl FnOnce(&mut World)) {
        change(&mut self.world.lock().expect("lock"));
        let _ = self.signals.try_send(Signal::DevicesChanged);
    }

    async fn next_event(&mut self) -> DeviceEvent {
        tokio::time::timeout(Duration::from_secs(5), self.events.recv())
            .await
            .expect("an event in time")
            .expect("the supervisor runs")
    }

    /// Nothing more is told within a while.
    async fn assert_quiet(&mut self) {
        let next = tokio::time::timeout(Duration::from_millis(150), self.events.recv()).await;
        assert!(next.is_err(), "unexpected {next:?}");
    }

    fn starts(&self) -> Vec<Route> {
        self.world.lock().expect("lock").starts.clone()
    }

    /// One device cycle through the running unit; `false` when no unit runs.
    fn render(&self, frames: usize) -> bool {
        let mut world = self.world.lock().expect("lock");
        let Some(parts) = world.active.as_mut() else {
            return false;
        };
        let mut out = vec![0.0; frames];
        parts.renderer.begin_cycle();
        parts.renderer.render(&mut out);
        parts.renderer.end_cycle();
        true
    }
}

fn unplug(world: &mut World, id: u32) {
    world
        .snapshot
        .devices
        .retain(|device| device.id != DeviceId(id));
}

#[tokio::test]
async fn an_unplugged_microphone_falls_back_and_comes_back() {
    let mut rig = rig(
        mac(&[SHURE], BUILT_IN_MIC, SPEAKERS),
        shure(),
        quick(),
        |_| {},
    )
    .expect("starts");
    let first = &rig.starts()[0];
    assert_eq!(first.input.device, DeviceId(SHURE));
    assert!(first.input.pinned && !first.output.pinned);
    rig.change(|world| unplug(world, SHURE));
    assert_eq!(
        rig.next_event().await,
        DeviceEvent::Missing {
            kind: DeviceKind::Input,
            wanted: "Shure MV7+".to_owned(),
            using: "MacBook Pro Microphone".to_owned(),
            why: Absence::Unplugged,
        }
    );
    assert!(
        matches!(rig.next_event().await, DeviceEvent::Switched { input, .. } if input == "MacBook Pro Microphone")
    );
    rig.change(|world| world.snapshot = mac(&[SHURE], BUILT_IN_MIC, SPEAKERS));
    assert_eq!(
        rig.next_event().await,
        DeviceEvent::Returned {
            kind: DeviceKind::Input,
            name: "Shure MV7+".to_owned(),
        }
    );
    assert!(matches!(
        rig.next_event().await,
        DeviceEvent::Switched { .. }
    ));
    let inputs: Vec<u32> = rig
        .starts()
        .iter()
        .map(|route| route.input.device.0)
        .collect();
    assert_eq!(inputs, [SHURE, BUILT_IN_MIC, SHURE]);
    assert_eq!(rig.world.lock().expect("lock").stops, 2);
}

#[tokio::test]
async fn the_system_default_is_followed_as_it_changes() {
    let mut rig = rig(
        mac(&[], BUILT_IN_MIC, SPEAKERS),
        InputChoice::SystemDefault,
        quick(),
        |_| {},
    )
    .expect("starts");
    // AirPods connect, and macOS makes them both defaults.
    rig.change(|world| world.snapshot = mac(&[HEADPHONES], HEADPHONES, HEADPHONES));
    assert_eq!(
        rig.next_event().await,
        DeviceEvent::Switched {
            input: "AirPods Pro".to_owned(),
            output: "AirPods Pro".to_owned(),
        }
    );
    // They disconnect: back to the Mac's own.
    rig.change(|world| world.snapshot = mac(&[], BUILT_IN_MIC, SPEAKERS));
    assert!(
        matches!(rig.next_event().await, DeviceEvent::Switched { output, .. } if output == "MacBook Pro Speakers")
    );
    rig.assert_quiet().await;
    assert_eq!(rig.starts().len(), 3);
}

#[tokio::test]
async fn a_burst_of_notifications_rebuilds_once_and_no_change_rebuilds_nothing() {
    // The settle window is 25 times the 20 ms gap: with 100 ms, a CI runner that stalled this
    // thread between the two notifications let the first one settle alone.
    let timing = Timing {
        settle: Duration::from_millis(500),
        settle_max: Duration::from_millis(2_000),
        ..quick()
    };
    let mut rig = rig(
        mac(&[], BUILT_IN_MIC, SPEAKERS),
        InputChoice::SystemDefault,
        timing,
        |_| {},
    )
    .expect("starts");
    // AirPods connect: macOS moves the default output, then the default input, a moment apart.
    rig.change(|world| world.snapshot = mac(&[HEADPHONES], BUILT_IN_MIC, HEADPHONES));
    std::thread::sleep(Duration::from_millis(20));
    rig.change(|world| world.snapshot = mac(&[HEADPHONES], HEADPHONES, HEADPHONES));
    assert_eq!(
        rig.next_event().await,
        DeviceEvent::Switched {
            input: "AirPods Pro".to_owned(),
            output: "AirPods Pro".to_owned(),
        },
        "one rebuild, after the burst"
    );
    rig.assert_quiet().await;
    assert_eq!(rig.starts().len(), 2);
    rig.change(|_| {});
    rig.assert_quiet().await;
    assert_eq!(rig.starts().len(), 2, "the same route is not rebuilt");
}

#[tokio::test]
async fn playback_carries_on_across_a_rebuild_with_the_new_latency() {
    let mut rig = rig(
        mac(&[], BUILT_IN_MIC, SPEAKERS),
        InputChoice::SystemDefault,
        quick(),
        |_| {},
    )
    .expect("starts");
    assert_eq!(rig.output.output_latency_frames(), 720, "30 ms at 24 kHz");
    assert_eq!(rig.output.push(&[0.5; 1_000]), 1_000);
    assert!(rig.render(300));
    rig.change(|world| {
        world.snapshot = mac(&[HEADPHONES], BUILT_IN_MIC, HEADPHONES);
        world.latency_seconds = 0.2;
    });
    assert!(matches!(
        rig.next_event().await,
        DeviceEvent::Switched { .. }
    ));
    assert!(rig.render(300));
    assert_eq!(rig.output.played_frames(), 600, "the counter goes on");
    assert_eq!(rig.output.output_latency_frames(), 4_800);
}

#[tokio::test]
async fn a_unit_that_will_not_start_is_retried_falls_back_and_is_reported_lost() {
    let mut rig = rig(
        mac(&[SHURE], BUILT_IN_MIC, SPEAKERS),
        shure(),
        quick(),
        |_| {},
    )
    .expect("starts");
    assert!(rig.render(10));
    // The Shure is there but refuses every attempt: the stand-ins take over, and with a silent
    // virtual default input that is the built-in microphone.
    rig.change(|world| {
        world.snapshot = mac(&[SHURE, HEADPHONES, MIXER], MIXER, HEADPHONES);
        world.refusals = AUDIO_REBUILD_ATTEMPTS_MAX;
    });
    assert_eq!(
        rig.next_event().await,
        DeviceEvent::Missing {
            kind: DeviceKind::Input,
            wanted: "Shure MV7+".to_owned(),
            using: "MacBook Pro Microphone".to_owned(),
            why: Absence::WontStart,
        }
    );
    assert!(matches!(
        rig.next_event().await,
        DeviceEvent::Switched { .. }
    ));
    // Nothing starts at all: lost, with the parts kept for later.
    rig.change(|world| {
        world.refuse_all = true;
        world.snapshot.default_output = Some(DeviceId(SPEAKERS));
    });
    assert!(matches!(
        rig.next_event().await,
        DeviceEvent::Lost {
            error: AudioError::Os { .. }
        }
    ));
    let attempts = rig.starts().len();
    assert_eq!(attempts, 1 + 4 + 2 * AUDIO_REBUILD_ATTEMPTS_MAX as usize);
    assert!(!rig.render(10), "no unit runs");
    // The devices change again and a unit starts: the counter is the same one.
    rig.change(|world| world.refuse_all = false);
    assert!(matches!(
        rig.next_event().await,
        DeviceEvent::Switched { .. }
    ));
    assert!(rig.render(10));
    assert_eq!(rig.output.played_frames(), 0, "nothing was queued");
}

#[tokio::test]
async fn a_unit_that_stops_rendering_is_rebuilt_then_reported_lost() {
    let timing = Timing {
        stall: Duration::from_millis(30),
        ..quick()
    };
    let mut rig = rig(
        mac(&[], BUILT_IN_MIC, SPEAKERS),
        InputChoice::SystemDefault,
        timing,
        |_| {},
    )
    .expect("starts");
    // The test never renders: the unit looks dead.
    for _ in 0..AUDIO_REBUILD_ATTEMPTS_MAX {
        assert!(matches!(
            rig.next_event().await,
            DeviceEvent::Switched { .. }
        ));
    }
    assert_eq!(
        rig.next_event().await,
        DeviceEvent::Lost {
            error: AudioError::Stalled
        }
    );
    rig.assert_quiet().await;
    assert_eq!(rig.starts().len(), 1 + AUDIO_REBUILD_ATTEMPTS_MAX as usize);
}

#[tokio::test]
async fn a_missing_device_at_start_is_said_once() {
    let mut rig = rig(mac(&[], BUILT_IN_MIC, SPEAKERS), shure(), quick(), |_| {}).expect("starts");
    assert!(matches!(
        rig.next_event().await,
        DeviceEvent::Missing {
            why: Absence::NotConnected,
            ..
        }
    ));
    rig.change(|_| {});
    rig.assert_quiet().await;
}

#[test]
fn a_first_start_that_fails_everywhere_is_an_error() {
    let failed = rig(
        mac(&[SHURE], BUILT_IN_MIC, SPEAKERS),
        shure(),
        quick(),
        |world| world.refuse_all = true,
    );
    assert!(matches!(failed, Err(AudioError::Os { .. })));
    let none = rig(
        DeviceSnapshot::default(),
        InputChoice::SystemDefault,
        quick(),
        |_| {},
    );
    assert!(matches!(none, Err(AudioError::NoInputDevice)));
}

#[test]
fn dropping_the_handle_stops_the_unit() {
    let mut rig = rig(
        mac(&[], BUILT_IN_MIC, SPEAKERS),
        InputChoice::SystemDefault,
        quick(),
        |_| {},
    )
    .expect("starts");
    drop(rig.handle.take());
    let world = rig.world.lock().expect("lock");
    assert_eq!(world.stops, 1);
    assert!(world.active.is_none());
}
