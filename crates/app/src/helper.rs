//! Short-lived helper processes the shell runs (`ps`, `open`, `osascript`, a harness's
//! `--version`), each with a deadline and bounded output. Blocks, so callers use the blocking
//! pool. No `unsafe`: a process past its deadline is killed through `std::process::Child`.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long `ps`, `open` or a harness's `--version` may run before it is killed.
pub const HELPER_PROCESS_TIMEOUT_MS: u64 = 1_000;

/// Output kept from one helper process, per stream: `ps` prints one short line, `osascript` one
/// path, `--version` one line.
pub const HELPER_OUTPUT_MAX_BYTES: u64 = 4_096;

/// How often a running helper process is checked for its exit. Waits measured in minutes (a
/// dialog) check less often, at most every [`HELPER_POLL_SLOW_MS`].
const HELPER_POLL_MS: u64 = 5;
const HELPER_POLL_SLOW_MS: u64 = 100;

/// How a helper process ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finished {
    pub success: bool,
    /// Up to [`HELPER_OUTPUT_MAX_BYTES`], lossily decoded.
    pub stdout: String,
    pub stderr: String,
}

/// Runs `command` for at most `timeout`, killing it past that. `None` when it couldn't start or
/// was killed. Its output is read once it has exited, so a helper must print little (a full
/// pipe would hold it until the deadline).
pub fn run_bounded(mut command: Command, timeout: Duration) -> Option<Finished> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let started = Instant::now();
    let deadline = started + timeout;
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
                let _killed = child.kill();
                let _reaped = child.wait();
                return None;
            }
        }
    };
    Some(Finished {
        success: exit.success(),
        stdout: read_bounded(child.stdout.take()),
        stderr: read_bounded(child.stderr.take()),
    })
}

fn read_bounded(stream: Option<impl Read>) -> String {
    let mut bytes = Vec::new();
    if let Some(stream) = stream {
        // A read error leaves what was read; the caller judges the text.
        let _read = stream.take(HELPER_OUTPUT_MAX_BYTES).read_to_end(&mut bytes);
    }
    String::from_utf8_lossy(&bytes).into_owned()
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
