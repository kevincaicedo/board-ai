//! Drives one harness subprocess: JSON lines in and out, bounded capture, and shutdown.
//!
//! The stream owns the child, so nothing is spawned and dropping the stream kills it. Cancel
//! is SIGINT to the child's process group, then SIGKILL after a grace period. Signals go
//! through `/bin/kill` instead of `libc::kill`: the runtime denies `unsafe`, and the child is
//! not reaped until we wait on it, so its process id (and group id) cannot be reused meanwhile.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use boardai_core::harness::{HarnessEvent, HarnessFailure};
use futures_util::stream;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::time::Instant;

use super::HarnessStream;
use super::git;
use super::lines::{LineSplitter, TailBuffer};
use crate::cancel::CancelToken;
use crate::limits::{
    HARNESS_CANCEL_GRACE_MS, HARNESS_EXIT_DRAIN_MS, HARNESS_FAILURE_DETAIL_MAX_BYTES,
    HARNESS_OUTPUT_TAIL_BYTES,
};

/// What to run, where.
#[derive(Clone, Debug)]
pub(crate) struct ProcessSpec {
    pub program: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: PathBuf,
    pub environment: Vec<(OsString, OsString)>,
}

/// Work done before the process starts.
#[derive(Clone, Debug)]
pub(crate) enum Preflight {
    None,
    /// A change task: refuse a dirty tree, then create and switch to the branch.
    Branch {
        branch: String,
    },
}

/// What a protocol asks of the driver after a line.
#[derive(Debug, Default)]
pub(crate) struct Output {
    pub events: Vec<HarnessEvent>,
    /// Lines to write to stdin (each gets its LF).
    pub replies: Vec<String>,
}

/// One harness's wire protocol, on top of the driver.
pub(crate) trait Protocol: Send + 'static {
    /// Lines written to stdin once the process starts.
    fn opening(&mut self) -> Vec<String>;

    /// One stdout line. A `Finished` or `Failed` event ends the task.
    fn on_line(&mut self, line: &[u8], output: &mut Output);

    /// The process ended before the protocol produced a terminal event.
    fn on_exit(&mut self, status: Option<ExitStatus>, stderr_tail: &str) -> HarnessEvent;
}

/// Runs a protocol over a process. The stream always ends with exactly one terminal event.
pub(crate) fn run<P: Protocol>(
    spec: ProcessSpec,
    preflight: Preflight,
    protocol: P,
    cancel: CancelToken,
) -> HarnessStream {
    let driver = Driver {
        spec,
        preflight,
        protocol,
        cancel,
        pending: VecDeque::new(),
        running: None,
        started: false,
        ended: false,
    };
    Box::pin(stream::unfold(driver, |mut driver| async move {
        let event = driver.next().await?;
        Some((event, driver))
    }))
}

struct Driver<P> {
    spec: ProcessSpec,
    preflight: Preflight,
    protocol: P,
    cancel: CancelToken,
    pending: VecDeque<HarnessEvent>,
    running: Option<Running>,
    started: bool,
    /// A terminal event has been queued; nothing follows it.
    ended: bool,
}

struct Running {
    child: Child,
    group: Option<u32>,
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
    splitter: LineSplitter,
    output: TailBuffer,
    stderr_tail: TailBuffer,
    reaped: bool,
}

enum Wake {
    Cancelled,
    Stdout(io::Result<usize>),
    Stderr(io::Result<usize>),
}

const READ_CHUNK_BYTES: usize = 16 * 1024;

impl<P: Protocol> Driver<P> {
    async fn next(&mut self) -> Option<HarnessEvent> {
        loop {
            // A terminal event is held until the process is gone, so when the consumer sees the
            // end of the task, nothing is left running.
            let terminal_waits = self.running.is_some() && self.pending.len() == 1 && self.ended;
            if !terminal_waits && let Some(event) = self.pending.pop_front() {
                return Some(event);
            }
            if self.ended && self.running.is_none() {
                return None;
            }
            if !self.started {
                self.started = true;
                self.start().await;
            } else if self.ended {
                let _ = self.wind_down(false).await;
            } else {
                self.pump().await;
            }
        }
    }

    fn end_with(&mut self, event: HarnessEvent) {
        if !self.ended {
            self.ended = true;
            self.pending.push_back(event);
        }
    }

    async fn start(&mut self) {
        if let Preflight::Branch { branch } = &self.preflight {
            let prepared = tokio::select! {
                biased;
                () = self.cancel.cancelled() => Err(git::GitError::Cancelled),
                prepared = git::prepare_branch(&self.spec.working_directory, branch) => prepared,
            };
            if let Err(error) = prepared {
                self.end_with(error.into_event());
                return;
            }
        }
        match spawn(&self.spec) {
            Ok(mut running) => {
                let opening = self.protocol.opening();
                if let Err(error) = write_lines(&mut running, &opening).await {
                    self.running = Some(running);
                    self.end_with(failed(
                        HarnessFailure::Crashed,
                        format!("could not write: {error}"),
                    ));
                    return;
                }
                self.running = Some(running);
            }
            Err(event) => self.end_with(event),
        }
    }

    async fn pump(&mut self) {
        let mut stdout_buffer = vec![0_u8; READ_CHUNK_BYTES];
        let mut stderr_buffer = vec![0_u8; READ_CHUNK_BYTES];
        let wake = {
            let Some(running) = self.running.as_mut() else {
                self.ended = true;
                return;
            };
            tokio::select! {
                biased;
                () = self.cancel.cancelled() => Wake::Cancelled,
                read = read_some(running.stdout.as_mut(), &mut stdout_buffer) => Wake::Stdout(read),
                read = read_some(running.stderr.as_mut(), &mut stderr_buffer) => Wake::Stderr(read),
            }
        };
        match wake {
            Wake::Cancelled => {
                let _ = self.wind_down(true).await;
                self.end_with(failed(HarnessFailure::Cancelled, "cancelled".to_owned()));
            }
            Wake::Stdout(Ok(read)) if read > 0 => self.on_stdout(&stdout_buffer[..read]).await,
            Wake::Stdout(Ok(_) | Err(_)) => self.on_stdout_closed().await,
            Wake::Stderr(Ok(read)) if read > 0 => self.on_stderr(&stderr_buffer[..read]),
            Wake::Stderr(Ok(_) | Err(_)) => {
                if let Some(running) = self.running.as_mut() {
                    running.stderr = None;
                }
            }
        }
    }

    async fn on_stdout(&mut self, bytes: &[u8]) {
        let Some(running) = self.running.as_mut() else {
            return;
        };
        running.output.push(bytes);
        let mut lines = Vec::new();
        running.splitter.push(bytes, &mut lines);
        self.handle_lines(lines).await;
    }

    fn on_stderr(&mut self, bytes: &[u8]) {
        if let Some(running) = self.running.as_mut() {
            running.output.push(bytes);
            running.stderr_tail.push(bytes);
        }
    }

    async fn handle_lines(&mut self, lines: Vec<Vec<u8>>) {
        for line in lines {
            if self.ended {
                return;
            }
            let mut output = Output::default();
            self.protocol.on_line(&line, &mut output);
            for event in output.events {
                if self.ended {
                    break;
                }
                let terminal = matches!(
                    event,
                    HarnessEvent::Finished { .. } | HarnessEvent::Failed { .. }
                );
                self.pending.push_back(event);
                self.ended = terminal;
            }
            if let Some(running) = self.running.as_mut()
                && !self.ended
                && let Err(error) = write_lines(running, &output.replies).await
            {
                tracing::debug!(%error, "a harness reply could not be written");
            }
        }
    }

    async fn on_stdout_closed(&mut self) {
        let mut lines = Vec::new();
        if let Some(running) = self.running.as_mut() {
            running.stdout = None;
            running.splitter.finish(&mut lines);
        }
        self.handle_lines(lines).await;
        let (status, stderr) = self.wind_down(false).await;
        if !self.ended {
            let event = self.protocol.on_exit(status, &stderr);
            self.end_with(event);
        }
    }

    /// Stops the process and reaps it. A finished task gets its stdin closed and a grace period
    /// to exit; a cancelled one gets SIGINT first. Either way it is killed if it lingers.
    /// Returns the exit status and the end of stderr.
    async fn wind_down(&mut self, interrupt_first: bool) -> (Option<ExitStatus>, String) {
        let Some(mut running) = self.running.take() else {
            return (None, String::new());
        };
        running.stdin = None;
        let grace = Duration::from_millis(HARNESS_CANCEL_GRACE_MS);
        // A cancel that arrives while a finished task winds down skips straight to the kill;
        // a cancel that caused the wind-down still gets its full grace period.
        let watch = (!self.cancel.is_cancelled()).then_some(&self.cancel);
        if interrupt_first {
            signal_group(running.group, "INT").await;
        }
        let mut exited = wait_draining(&mut running, grace, watch).await;
        if !exited && !interrupt_first && !self.cancel.is_cancelled() {
            signal_group(running.group, "INT").await;
            exited = wait_draining(&mut running, grace, watch).await;
        }
        if !exited {
            signal_group(running.group, "KILL").await;
            let _ = running.child.kill().await;
            running.reaped = true;
        }
        let status = running.child.try_wait().ok().flatten();
        tracing::debug!(
            ?status,
            captured_bytes = running.output.len(),
            skipped_lines = running.splitter.skipped_lines(),
            "harness process ended"
        );
        (
            status,
            running
                .stderr_tail
                .tail_text(HARNESS_FAILURE_DETAIL_MAX_BYTES),
        )
    }
}

fn spawn(spec: &ProcessSpec) -> Result<Running, HarnessEvent> {
    let mut command = Command::new(&spec.program);
    command
        .args(&spec.arguments)
        .current_dir(&spec.working_directory)
        .envs(spec.environment.iter().map(|(key, value)| (key, value)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        // Its own group, so a signal reaches the tools it spawns too.
        .process_group(0);
    let mut child = command.spawn().map_err(|error| {
        let reason = if error.kind() == io::ErrorKind::NotFound {
            HarnessFailure::NotInstalled
        } else {
            HarnessFailure::Crashed
        };
        failed(
            reason,
            format!("{} could not start: {error}", spec.program.display()),
        )
    })?;
    Ok(Running {
        group: child.id(),
        stdin: child.stdin.take(),
        stdout: child.stdout.take(),
        stderr: child.stderr.take(),
        child,
        splitter: LineSplitter::default(),
        output: TailBuffer::new(HARNESS_OUTPUT_TAIL_BYTES),
        stderr_tail: TailBuffer::new(HARNESS_OUTPUT_TAIL_BYTES / 16),
        reaped: false,
    })
}

async fn write_lines(running: &mut Running, lines: &[String]) -> io::Result<()> {
    let Some(stdin) = running.stdin.as_mut() else {
        return Ok(());
    };
    for line in lines {
        stdin.write_all(line.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
    }
    stdin.flush().await
}

async fn read_some(
    reader: Option<&mut (impl AsyncRead + Unpin)>,
    buffer: &mut [u8],
) -> io::Result<usize> {
    match reader {
        Some(reader) => reader.read(buffer).await,
        None => std::future::pending().await,
    }
}

/// Waits for the child to exit while still reading its pipes (a child blocked on a full pipe
/// would never exit). Returns whether it exited in time; `cancel`, when given, cuts the wait.
async fn wait_draining(
    running: &mut Running,
    limit: Duration,
    cancel: Option<&CancelToken>,
) -> bool {
    let deadline = Instant::now() + limit;
    let mut stdout_buffer = vec![0_u8; READ_CHUNK_BYTES];
    let mut stderr_buffer = vec![0_u8; READ_CHUNK_BYTES];
    loop {
        tokio::select! {
            status = running.child.wait() => {
                running.reaped = status.is_ok();
                drain_after_exit(running).await;
                return true;
            }
            read = read_some(running.stdout.as_mut(), &mut stdout_buffer) => match read {
                Ok(read) if read > 0 => running.output.push(&stdout_buffer[..read]),
                Ok(_) | Err(_) => running.stdout = None,
            },
            read = read_some(running.stderr.as_mut(), &mut stderr_buffer) => match read {
                Ok(read) if read > 0 => {
                    running.output.push(&stderr_buffer[..read]);
                    running.stderr_tail.push(&stderr_buffer[..read]);
                }
                Ok(_) | Err(_) => running.stderr = None,
            },
            () = tokio::time::sleep_until(deadline) => return false,
            () = cancelled(cancel) => return false,
        }
    }
}

/// Reads what the exited child left in its pipes. Exit and the last stderr bytes race: without
/// this, a failure's detail could miss the harness's own error message. A grandchild can hold a
/// pipe open, so the wait is bounded.
async fn drain_after_exit(running: &mut Running) {
    let deadline = Instant::now() + Duration::from_millis(HARNESS_EXIT_DRAIN_MS);
    let mut stdout_buffer = vec![0_u8; READ_CHUNK_BYTES];
    let mut stderr_buffer = vec![0_u8; READ_CHUNK_BYTES];
    while running.stdout.is_some() || running.stderr.is_some() {
        tokio::select! {
            read = read_some(running.stdout.as_mut(), &mut stdout_buffer) => match read {
                Ok(read) if read > 0 => running.output.push(&stdout_buffer[..read]),
                Ok(_) | Err(_) => running.stdout = None,
            },
            read = read_some(running.stderr.as_mut(), &mut stderr_buffer) => match read {
                Ok(read) if read > 0 => {
                    running.output.push(&stderr_buffer[..read]);
                    running.stderr_tail.push(&stderr_buffer[..read]);
                }
                Ok(_) | Err(_) => running.stderr = None,
            },
            () = tokio::time::sleep_until(deadline) => return,
        }
    }
}

async fn cancelled(cancel: Option<&CancelToken>) {
    match cancel {
        Some(cancel) => cancel.cancelled().await,
        None => std::future::pending().await,
    }
}

/// Signals the child's process group with `/bin/kill`. Skipped once the child is reaped.
async fn signal_group(group: Option<u32>, signal: &str) {
    let Some(group) = group else { return };
    let status = tokio::time::timeout(
        Duration::from_secs(1),
        Command::new("/bin/kill")
            .arg(format!("-{signal}"))
            .arg("--")
            .arg(format!("-{group}"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status(),
    )
    .await;
    tracing::debug!(signal, ?status, "signalled the harness process group");
}

impl Drop for Running {
    /// The stream was dropped mid-task: `kill_on_drop` kills the child, and this kills the
    /// rest of its group. It runs `/bin/kill` synchronously; that takes a few milliseconds.
    fn drop(&mut self) {
        if self.reaped {
            return;
        }
        if let Some(group) = self.group {
            let _ = std::process::Command::new("/bin/kill")
                .arg("-KILL")
                .arg("--")
                .arg(format!("-{group}"))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

pub(crate) fn failed(reason: HarnessFailure, detail: String) -> HarnessEvent {
    HarnessEvent::Failed { reason, detail }
}
