//! The device supervisor: one thread that owns the audio unit and rebuilds it when the devices
//! change (LLD §3.4, "Devices").
//!
//! CoreAudio's listeners only signal it. It waits for the notifications to settle, plans the
//! route from a fresh snapshot ([`devices::plan`]), and rebuilds the unit when the route
//! differs. A rebuild moves the mic writer and the playback renderer to the new unit, so the
//! rings, the played counter and the clause timeline carry on: playback pauses for the gap and
//! resumes where it was. The new device's latency is published to the voice output.
//!
//! It also watches the renderer: a unit whose playback callback stops running (a device that
//! died without a notification) is rebuilt too. Everything is bounded: the settle window,
//! the start attempts (the planned route, then the stand-ins), the stall rebuilds. When nothing
//! can start, it reports the audio lost and waits for the devices to change.

use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use super::devices::{self, DeviceEvent, DeviceSnapshot, OutputChoice, Plan, Route};
use super::{AudioError, InputChoice, MicWriter, PlaybackMonitor, PlaybackRenderer, VOICE_RATE_HZ};
use crate::limits::{
    AUDIO_DEVICE_SETTLE_MAX_MS, AUDIO_DEVICE_SETTLE_MS, AUDIO_REBUILD_ATTEMPTS_MAX,
    AUDIO_REBUILD_RETRY_MS, AUDIO_STALL_MS, AUDIO_WATCHDOG_INTERVAL_MS,
};

/// What wakes the supervisor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Signal {
    DevicesChanged,
    Shutdown,
}

/// The ring ends a unit drives. They outlive every unit: each rebuild moves them on.
pub(crate) struct UnitParts {
    pub(crate) mic: MicWriter,
    pub(crate) renderer: PlaybackRenderer,
}

/// What a unit measured when it started.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct UnitFacts {
    pub unit_rate_hz: u32,
    /// From a frame leaving the voice ring to the speaker, conversion included.
    pub output_latency_seconds: f64,
    /// Whether other apps' audio is ducked as little as macOS allows.
    pub other_audio_ducking_minimized: bool,
}

/// The facts of the running unit, read by `AudioIo`; `None` while none runs.
pub(crate) type SharedFacts = Arc<Mutex<Option<UnitFacts>>>;

/// The device side: CoreAudio in the app, fakes in tests.
pub(crate) trait AudioBackend: Send + 'static {
    type Unit: Send + 'static;

    fn snapshot(&mut self) -> DeviceSnapshot;

    /// Starts a unit on `route`. A failure hands the parts back.
    fn start(
        &mut self,
        route: &Route,
        parts: UnitParts,
    ) -> Result<(Self::Unit, UnitFacts), (AudioError, UnitParts)>;

    /// Stops a unit; once this returns no callback runs, and the parts come back.
    fn stop(&mut self, unit: Self::Unit) -> UnitParts;
}

/// The supervisor's clock settings: the limits in the app, shorter in tests.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Timing {
    pub(crate) settle: Duration,
    pub(crate) settle_max: Duration,
    pub(crate) watchdog: Duration,
    pub(crate) stall: Duration,
    pub(crate) retry: Duration,
}

impl Timing {
    pub(crate) const fn from_limits() -> Self {
        Self {
            settle: Duration::from_millis(AUDIO_DEVICE_SETTLE_MS),
            settle_max: Duration::from_millis(AUDIO_DEVICE_SETTLE_MAX_MS),
            watchdog: Duration::from_millis(AUDIO_WATCHDOG_INTERVAL_MS),
            stall: Duration::from_millis(AUDIO_STALL_MS),
            retry: Duration::from_millis(AUDIO_REBUILD_RETRY_MS),
        }
    }
}

enum Running<U> {
    Unit {
        unit: U,
        route: Route,
    },
    /// No unit runs (it could not start); the parts wait for the next device change.
    Stopped(UnitParts),
}

/// Render progress, to tell a dead unit from a quiet one.
struct Watchdog {
    cycles: u64,
    since: Instant,
    stall_rebuilds: u32,
}

pub(crate) struct Supervisor<B: AudioBackend> {
    backend: B,
    input: InputChoice,
    output: OutputChoice,
    /// Always `Some` between calls; `None` only while a rebuild holds the parts.
    running: Option<Running<B::Unit>>,
    plan: Option<Plan>,
    monitor: PlaybackMonitor,
    events: mpsc::Sender<DeviceEvent>,
    facts: SharedFacts,
    timing: Timing,
    watchdog: Watchdog,
    /// The audio was reported lost; nothing is retried until the devices change.
    lost: bool,
    /// Why the last start failed, for a first start that fails.
    last_error: Option<AudioError>,
}

impl<B: AudioBackend> Supervisor<B> {
    /// Starts the first unit, falling back to the stand-ins. Fails when nothing starts, so the
    /// caller can do without audio.
    pub(crate) fn start(
        mut backend: B,
        choices: (InputChoice, OutputChoice),
        parts: UnitParts,
        events: mpsc::Sender<DeviceEvent>,
        facts: SharedFacts,
        timing: Timing,
    ) -> Result<Self, AudioError> {
        let snapshot = backend.snapshot();
        let (input, output) = choices;
        let plan = devices::plan(&input, &output, &snapshot)?;
        let monitor = parts.renderer.monitor();
        let mut supervisor = Self {
            backend,
            input,
            output,
            running: Some(Running::Stopped(parts)),
            plan: None,
            watchdog: Watchdog {
                cycles: monitor.render_cycles(),
                since: Instant::now(),
                stall_rebuilds: 0,
            },
            monitor,
            events,
            facts,
            timing,
            lost: false,
            last_error: None,
        };
        supervisor.apply(plan, &snapshot, false);
        match supervisor.running {
            Some(Running::Unit { .. }) => Ok(supervisor),
            Some(Running::Stopped(_)) | None => Err(supervisor
                .last_error
                .take()
                .unwrap_or(AudioError::VoiceProcessingUnavailable)),
        }
    }

    /// Runs until `Shutdown`, then stops the unit.
    pub(crate) fn run(mut self, signals: &Receiver<Signal>) {
        loop {
            match signals.recv_timeout(self.timing.watchdog) {
                Ok(Signal::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(Signal::DevicesChanged) => {
                    if !self.settle(signals) {
                        break;
                    }
                    self.lost = false;
                    self.watchdog.stall_rebuilds = 0;
                    self.check(false);
                }
                Err(RecvTimeoutError::Timeout) => {
                    if self.stalled(Instant::now()) {
                        self.check(true);
                    }
                }
            }
        }
        if let Some(Running::Unit { unit, .. }) = self.running.take() {
            drop(self.backend.stop(unit));
        }
    }

    /// Waits for the notifications to stop, within the settle limits. `false` on shutdown.
    fn settle(&self, signals: &Receiver<Signal>) -> bool {
        let deadline = Instant::now() + self.timing.settle_max;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return true;
            }
            match signals.recv_timeout(self.timing.settle.min(left)) {
                Ok(Signal::DevicesChanged) => {}
                Ok(Signal::Shutdown) | Err(RecvTimeoutError::Disconnected) => return false,
                Err(RecvTimeoutError::Timeout) => return true,
            }
        }
    }

    /// Plans from a fresh snapshot and rebuilds when the route changed (or the unit stalled).
    fn check(&mut self, stalled: bool) {
        let snapshot = self.backend.snapshot();
        match devices::plan(&self.input, &self.output, &snapshot) {
            Ok(plan) => self.apply(plan, &snapshot, stalled),
            Err(error) => {
                self.stop_unit();
                self.lose(error);
            }
        }
    }

    fn apply(&mut self, plan: Plan, snapshot: &DeviceSnapshot, force: bool) {
        for event in devices::changes(self.plan.as_ref(), &plan) {
            self.emit(event);
        }
        let route = plan.route.clone();
        let first = self.plan.is_none();
        self.plan = Some(plan);
        let same = matches!(&self.running, Some(Running::Unit { route: running, .. }) if *running == route);
        if same && !force {
            return;
        }
        let mut candidates = vec![route.clone()];
        if let Ok(stand_ins) = Route::stand_ins(snapshot)
            && stand_ins != route
        {
            candidates.push(stand_ins);
        }
        self.rebuild(&candidates, !first);
    }

    /// Stops the running unit and starts one on the first candidate that starts.
    fn rebuild(&mut self, candidates: &[Route], announce: bool) {
        let Some(mut parts) = self.take_parts() else {
            debug_assert!(false, "the supervisor lost its parts");
            return;
        };
        let mut last_error = AudioError::VoiceProcessingUnavailable;
        for route in candidates {
            for attempt in 0..AUDIO_REBUILD_ATTEMPTS_MAX {
                if attempt > 0 {
                    // Bounded: at most two short waits per route; a shutdown waits for them.
                    std::thread::sleep(self.timing.retry);
                }
                match self.backend.start(route, parts) {
                    Ok((unit, facts)) => {
                        if let Some(planned) = candidates.first() {
                            for event in devices::refused(planned, route) {
                                self.emit(event);
                            }
                        }
                        self.started(unit, facts, route.clone(), announce);
                        return;
                    }
                    Err((error, back)) => {
                        tracing::warn!(%error, attempt, "the audio unit would not start");
                        parts = back;
                        last_error = error;
                    }
                }
            }
        }
        self.running = Some(Running::Stopped(parts));
        self.lose(last_error);
    }

    fn started(&mut self, unit: B::Unit, facts: UnitFacts, route: Route, announce: bool) {
        let latency_frames = (facts.output_latency_seconds * f64::from(VOICE_RATE_HZ)).round();
        self.monitor
            .set_output_latency_frames(latency_frames.max(0.0) as u64);
        if let Ok(mut shared) = self.facts.lock() {
            *shared = Some(facts);
        }
        if announce {
            self.emit(DeviceEvent::Switched {
                input: route.input.name.clone(),
                output: route.output.name.clone(),
            });
        }
        self.running = Some(Running::Unit { unit, route });
        self.lost = false;
        self.watchdog.cycles = self.monitor.render_cycles();
        self.watchdog.since = Instant::now();
    }

    /// Whether the running unit stopped rendering for the stall limit. Gives up, out loud,
    /// after the rebuild limit.
    fn stalled(&mut self, now: Instant) -> bool {
        let cycles = self.monitor.render_cycles();
        if cycles != self.watchdog.cycles {
            self.watchdog = Watchdog {
                cycles,
                since: now,
                stall_rebuilds: 0,
            };
            return false;
        }
        let running = matches!(self.running, Some(Running::Unit { .. }));
        if !running || self.lost || now < self.watchdog.since + self.timing.stall {
            return false;
        }
        if self.watchdog.stall_rebuilds >= AUDIO_REBUILD_ATTEMPTS_MAX {
            self.lose(AudioError::Stalled);
            return false;
        }
        self.watchdog.stall_rebuilds += 1;
        self.watchdog.since = now;
        tracing::warn!("the audio unit stopped rendering; rebuilding it");
        true
    }

    fn take_parts(&mut self) -> Option<UnitParts> {
        match self.running.take()? {
            Running::Unit { unit, .. } => Some(self.backend.stop(unit)),
            Running::Stopped(parts) => Some(parts),
        }
    }

    fn stop_unit(&mut self) {
        if let Some(parts) = self.take_parts() {
            self.running = Some(Running::Stopped(parts));
        }
        if let Ok(mut shared) = self.facts.lock() {
            *shared = None;
        }
    }

    fn lose(&mut self, error: AudioError) {
        self.last_error = Some(error.clone());
        if !self.lost {
            self.lost = true;
            self.emit(DeviceEvent::Lost { error });
        }
    }

    fn emit(&self, event: DeviceEvent) {
        tracing::info!(?event, "audio devices");
        if self.events.try_send(event).is_err() {
            tracing::warn!("a device event was dropped: nobody is reading them fast enough");
        }
    }
}

/// The supervisor's thread. Dropping it stops the unit and waits for the thread.
pub(crate) struct SupervisorHandle {
    signals: SyncSender<Signal>,
    thread: Option<JoinHandle<()>>,
}

impl SupervisorHandle {
    pub(crate) fn spawn<B: AudioBackend>(
        supervisor: Supervisor<B>,
        signals: SyncSender<Signal>,
        receiver: Receiver<Signal>,
    ) -> Result<Self, AudioError> {
        let thread = std::thread::Builder::new()
            .name("boardai-audio-devices".to_owned())
            .spawn(move || supervisor.run(&receiver))
            .map_err(|error| AudioError::Thread {
                detail: error.to_string(),
            })?;
        Ok(Self {
            signals,
            thread: Some(thread),
        })
    }
}

impl Drop for SupervisorHandle {
    fn drop(&mut self) {
        // Waits at most for a rebuild in progress (bounded by its attempts).
        let _ = self.signals.send(Signal::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
