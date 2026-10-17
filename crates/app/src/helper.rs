//! Short-lived helper processes the shell runs (`ps`, `open`, `osascript`, a harness's
//! `--version`), each with a deadline and bounded output. Blocks, so callers use the blocking
//! pool. No `unsafe`: a helper runs in its own process group, which is killed through
//! `/bin/kill` (as the harness does, D-024) past its deadline.
//!
//! The deadline bounds the whole call (law 5): both pipes drain on their own threads while the
//! helper runs, so a full pipe never holds it, and a background child that inherited a pipe
//! (`sh -c "daemon & echo 1.2"`) can't keep the call open: past [`HELPER_DRAIN_MS`] after the
//! helper exits, or past the deadline, its group is killed and what was read is kept.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// How long `ps`, `open` or a harness's `--version` may run before it is killed.
pub const HELPER_PROCESS_TIMEOUT_MS: u64 = 1_000;

/// Output kept from one helper process, per stream: `ps` prints one short line, `osascript` one
/// path, `--version` one line. The rest is read and dropped, so the helper never blocks on a
/// full pipe.
pub const HELPER_OUTPUT_MAX_BYTES: u64 = 4_096;

/// How long a helper's pipes may stay open after it exits (a background child holding them)
/// before its process group is killed, within the deadline. Output it wrote before exiting is
/// already in the pipe and is read at once.
pub const HELPER_DRAIN_MS: u64 = 200;

/// How often a running helper process is checked for its exit. Waits measured in minutes (a
/// dialog) check less often, at most every [`HELPER_POLL_SLOW_MS`].
const HELPER_POLL_MS: u64 = 5;
const HELPER_POLL_SLOW_MS: u64 = 100;

/// Bytes read from a pipe at a time.
const HELPER_READ_CHUNK_BYTES: usize = 1_024;

/// How a helper process ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finished {
    pub success: bool,
    /// Up to [`HELPER_OUTPUT_MAX_BYTES`], lossily decoded.
    pub stdout: String,
    pub stderr: String,
}

/// Runs `command` for at most `timeout`, killing it (and its process group) past that. `None`
/// when it couldn't start or was killed before it exited. Returns within `timeout` plus the
/// time to signal the group, whatever its children do with its pipes.
pub fn run_bounded(mut command: Command, timeout: Duration) -> Option<Finished> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .ok()?;
    let started = Instant::now();
    let deadline = started + timeout;
    let stdout = Drain::start(child.stdout.take());
    let stderr = Drain::start(child.stderr.take());
    let exit = loop {
        match child.try_wait() {
            Ok(Some(exit)) => break exit,
            Ok(None) if Instant::now() < deadline => {
                let poll = if started.elapsed() < Duration::from_secs(1) {
                    HELPER_POLL_MS
                } else {
                    HELPER_POLL_SLOW_MS
                };
                std::thread::sleep(Duration::from_millis(poll));
            }
            Ok(None) | Err(_) => {
                // Not reaped yet, so its group id can't have been reused.
                kill_group(child.id());
                let _killed = child.kill();
                let _reaped = child.wait();
                return None;
            }
        }
    };
    let drained_by = deadline.min(Instant::now() + Duration::from_millis(HELPER_DRAIN_MS));
    let out_ended = stdout.wait_until(drained_by);
    let err_ended = stderr.wait_until(drained_by);
    if !(out_ended && err_ended) {
        // A process still holds a pipe; one that stayed in the group keeps its id in use.
        tracing::debug!("a helper's pipe outlived it; its process group is killed");
        kill_group(child.id());
    }
    Some(Finished {
        success: exit.success(),
        stdout: stdout.take(),
        stderr: stderr.take(),
    })
}

/// SIGKILL to the helper's process group (its id: `process_group(0)` made it the leader).
fn kill_group(group: u32) {
    let killed = Command::new("/bin/kill")
        .arg("-KILL")
        .arg("--")
        .arg(format!("-{group}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    tracing::debug!(?killed, "signalled a helper's process group");
}

/// One pipe read to its end on its own thread, keeping the first [`HELPER_OUTPUT_MAX_BYTES`].
/// The thread ends at end of input, which comes at the latest when the group is killed (a
/// process that left the group and still holds the pipe keeps only that thread, blocked in a
/// read, until it exits).
struct Drain {
    shared: Arc<(Mutex<Drained>, Condvar)>,
}

#[derive(Default)]
struct Drained {
    bytes: Vec<u8>,
    ended: bool,
}

impl Drain {
    fn start(stream: Option<impl Read + Send + 'static>) -> Self {
        let shared = Arc::new((Mutex::new(Drained::default()), Condvar::new()));
        let Some(mut stream) = stream else {
            shared
                .0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .ended = true;
            return Self { shared };
        };
        let writer = Arc::clone(&shared);
        let spawned = std::thread::Builder::new()
            .name("helper-pipe".to_owned())
            .spawn(move || {
                let mut chunk = [0_u8; HELPER_READ_CHUNK_BYTES];
                let max = usize::try_from(HELPER_OUTPUT_MAX_BYTES).unwrap_or(usize::MAX);
                loop {
                    // A read error ends the pipe like its end does; the caller judges the text.
                    let read = stream.read(&mut chunk).unwrap_or(0);
                    let (lock, ended) = &*writer;
                    let mut drained = lock.lock().unwrap_or_else(PoisonError::into_inner);
                    if read == 0 {
                        drained.ended = true;
                        ended.notify_all();
                        return;
                    }
                    let room = max.saturating_sub(drained.bytes.len());
                    drained.bytes.extend_from_slice(&chunk[..read.min(room)]);
                }
            });
        if spawned.is_err() {
            tracing::warn!("a helper's output can't be read: no thread");
            shared
                .0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .ended = true;
        }
        Self { shared }
    }

    /// Waits for the pipe's end until `deadline`; `true` when it came.
    fn wait_until(&self, deadline: Instant) -> bool {
        let (lock, ended) = &*self.shared;
        let mut drained = lock.lock().unwrap_or_else(PoisonError::into_inner);
        while !drained.ended {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            drained = ended
                .wait_timeout(drained, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }

    /// What was read so far, lossily decoded.
    fn take(&self) -> String {
        let (lock, _) = &*self.shared;
        let bytes = std::mem::take(&mut lock.lock().unwrap_or_else(PoisonError::into_inner).bytes);
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::time::{Duration, Instant};

    use super::{HELPER_OUTPUT_MAX_BYTES, run_bounded};

    #[test]
    fn output_and_failure_come_back_bounded() {
        let mut echo = Command::new("/bin/sh");
        echo.args(["-c", "echo out; echo err >&2; exit 3"]);
        let finished = run_bounded(echo, Duration::from_secs(5)).expect("it ran");
        assert!(!finished.success);
        assert_eq!(finished.stdout, "out\n");
        assert_eq!(finished.stderr, "err\n");
        let mut long = Command::new("/bin/sh");
        long.args(["-c", "head -c 3000 /dev/zero | tr '\\0' x; printf yy"]);
        let finished = run_bounded(long, Duration::from_secs(5)).expect("it ran");
        assert!(finished.stdout.len() as u64 <= HELPER_OUTPUT_MAX_BYTES);
    }

    /// Review 2: a child that exits at once but leaves a background child holding its stdout
    /// kept the call open until that child exited, far past the deadline (law 5).
    #[test]
    fn the_deadline_bounds_the_whole_call_even_with_an_inherited_pipe() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "/bin/sleep 6 & echo 2.1.301"]);
        let started = Instant::now();
        let _finished = run_bounded(command, Duration::from_millis(1_000));
        let took = started.elapsed();
        assert!(
            took < Duration::from_secs(3),
            "run_bounded with a 1 s deadline returned after {took:?}"
        );
    }

    /// The background child that held the pipe goes with the helper's process group, and what
    /// the helper printed before it exited is kept.
    #[test]
    fn a_background_child_holding_a_pipe_is_killed_with_its_group() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "/bin/sleep 30 & echo $!"]);
        let finished = run_bounded(command, Duration::from_secs(5)).expect("it ran");
        assert!(finished.success);
        let pid = finished.stdout.trim().to_owned();
        assert!(pid.parse::<u32>().is_ok(), "{finished:?}");
        let alive = || {
            Command::new("/bin/kill")
                .args(["-0", &pid])
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        };
        let gone_by = Instant::now() + Duration::from_secs(2);
        while alive() && Instant::now() < gone_by {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!alive(), "the background sleep outlived the helper");
    }

    #[test]
    fn a_helper_past_its_deadline_is_killed_and_reports_nothing() {
        let mut sleep = Command::new("/bin/sleep");
        sleep.arg("30");
        let started = Instant::now();
        assert_eq!(run_bounded(sleep, Duration::from_millis(50)), None);
        assert!(started.elapsed() < Duration::from_secs(5));
        let missing = Command::new("/nonexistent/helper");
        assert_eq!(run_bounded(missing, Duration::from_secs(1)), None);
    }
}
