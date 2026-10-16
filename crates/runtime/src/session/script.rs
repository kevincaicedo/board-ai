//! Dev scripts: a list of utterances said as typed speech, timed against what BoardAI is doing,
//! for repeatable runs of the text dev loop and the app (`--script`, `BOARDAI_DEV_SCRIPT`).
//!
//! ```text
//! # Comments and blank lines are skipped.
//! Walk me through how a request reaches our API.
//! @4000 wait, why Redis first?
//! ```
//!
//! A plain line is said once BoardAI has finished answering the previous one. `@<ms>` says it
//! that long after BoardAI started speaking its answer to the previous line (a timed barge-in),
//! or as soon as it finishes if it never speaks. The first line is said at once.

use std::fmt;
use std::time::Duration;

use boardai_core::effect::{ForgetScope, LogRecord, Milestone};
use boardai_core::ids::CaptureId;
use boardai_core::input::Input;
use boardai_core::memory::MemoryWrite;
use boardai_core::protocol::{OrbState, UiEvent};
use boardai_core::time::SessionTime;
use boardai_core::tools::ToolOutput;
use futures_util::future::BoxFuture;
use tokio::sync::watch;

use super::inbox::{SessionClosed, SpeechInLink};
use super::ports::{SessionClose, SessionOpened, StoreRead, StoreSink};
use super::typed::say_typed;
use crate::limits::{SCRIPT_LINE_MAX_BYTES, SCRIPT_LINES_MAX, SCRIPT_WAIT_MAX_MS};

/// When a line is said.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cue {
    /// Once the answer to the previous line has finished.
    WhenIdle,
    /// This long after the answer to the previous line started playing.
    IntoSpeech { delay_ms: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptLine {
    pub cue: Cue,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Script {
    pub lines: Vec<ScriptLine>,
}

/// Why a script was rejected, with its 1-based line number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScriptError {
    TooManyLines,
    LineTooLong { line: usize },
    BadDelay { line: usize },
}

impl fmt::Display for ScriptError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyLines => {
                write!(formatter, "a script has at most {SCRIPT_LINES_MAX} lines")
            }
            Self::LineTooLong { line } => write!(
                formatter,
                "line {line} is over {SCRIPT_LINE_MAX_BYTES} bytes"
            ),
            Self::BadDelay { line } => {
                write!(
                    formatter,
                    "line {line}: `@` must be followed by milliseconds and text"
                )
            }
        }
    }
}

impl std::error::Error for ScriptError {}

impl Script {
    pub fn parse(text: &str) -> Result<Self, ScriptError> {
        let mut lines = Vec::new();
        for (index, raw) in text.lines().enumerate() {
            let line = index + 1;
            let raw = raw.trim();
            if raw.is_empty() || raw.starts_with('#') {
                continue;
            }
            if lines.len() >= SCRIPT_LINES_MAX {
                return Err(ScriptError::TooManyLines);
            }
            if raw.len() > SCRIPT_LINE_MAX_BYTES {
                return Err(ScriptError::LineTooLong { line });
            }
            lines.push(parse_line(raw, line)?);
        }
        Ok(Self { lines })
    }
}

fn parse_line(raw: &str, line: usize) -> Result<ScriptLine, ScriptError> {
    let Some(timed) = raw.strip_prefix('@') else {
        return Ok(ScriptLine {
            cue: Cue::WhenIdle,
            text: raw.to_owned(),
        });
    };
    let (delay, text) = timed
        .split_once(char::is_whitespace)
        .ok_or(ScriptError::BadDelay { line })?;
    let delay_ms = delay.parse().map_err(|_| ScriptError::BadDelay { line })?;
    // `raw` is trimmed, so text after the delay is never empty.
    Ok(ScriptLine {
        cue: Cue::IntoSpeech { delay_ms },
        text: text.trim().to_owned(),
    })
}

/// What a script waits on, counted from the core's own events.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    /// Times the orb came back to idle from anything else: an answer finished.
    pub answers_finished: u64,
    /// Times an answer's first audio played.
    pub answers_started: u64,
    pub orb: Option<OrbState>,
}

/// A [`StoreSink`] that also counts [`Progress`] from what passes through it.
pub struct Observed<S> {
    inner: S,
    progress: watch::Sender<Progress>,
}

impl<S: StoreSink> Observed<S> {
    #[must_use]
    pub fn new(inner: S) -> (Self, watch::Receiver<Progress>) {
        let (progress, receiver) = watch::channel(Progress::default());
        (Self { inner, progress }, receiver)
    }
}

impl<S: StoreSink> StoreSink for Observed<S> {
    fn report_to(&self, link: super::StoreLink) {
        self.inner.report_to(link);
    }

    fn input(&self, at: SessionTime, input: &Input) {
        self.inner.input(at, input);
    }

    fn ui_event(&self, at: SessionTime, event: &UiEvent) {
        if let UiEvent::Orb { state, .. } = event {
            self.progress.send_modify(|progress| {
                let was_busy = progress.orb.is_some_and(|orb| orb != OrbState::Idle);
                if *state == OrbState::Idle && was_busy {
                    progress.answers_finished += 1;
                }
                progress.orb = Some(*state);
            });
        }
        self.inner.ui_event(at, event);
    }

    fn log(&self, at: SessionTime, record: &LogRecord) {
        if let LogRecord::Milestone {
            milestone: Milestone::FirstAudio,
        } = record
        {
            self.progress
                .send_modify(|progress| progress.answers_started += 1);
        }
        self.inner.log(at, record);
    }

    fn remember(&self, at: SessionTime, memory: &MemoryWrite) {
        self.inner.remember(at, memory);
    }

    fn forget(&self, at: SessionTime, scope: ForgetScope) {
        self.inner.forget(at, scope);
    }

    fn accept_proposal(&self, at: SessionTime, capture: CaptureId) {
        self.inner.accept_proposal(at, capture);
    }

    fn read(&self, read: StoreRead) -> BoxFuture<'static, ToolOutput> {
        self.inner.read(read)
    }

    fn start_session(&self) -> BoxFuture<'static, SessionOpened> {
        self.inner.start_session()
    }

    fn finish_session(&self, close: SessionClose) -> BoxFuture<'static, ()> {
        self.inner.finish_session(close)
    }

    fn flush(&self) -> BoxFuture<'static, ()> {
        self.inner.flush()
    }
}

/// Says every line at its cue, then waits for the last answer to finish. Each wait is bounded
/// by [`SCRIPT_WAIT_MAX_MS`]; past it the script goes on, with a warning.
pub async fn run_script(
    script: Script,
    link: &SpeechInLink,
    progress: &mut watch::Receiver<Progress>,
) -> Result<(), SessionClosed> {
    let mut before: Option<Progress> = None;
    for line in &script.lines {
        if let Some(before) = before {
            match line.cue {
                Cue::WhenIdle => wait_for(progress, |now| finished(before, now)).await,
                Cue::IntoSpeech { delay_ms } => {
                    let started = |now: &Progress| started(before, now) || finished(before, now);
                    wait_for(progress, started).await;
                    let answered = finished(before, &progress.borrow());
                    if !answered {
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    }
                }
            }
        }
        say_typed(link, &line.text).await?;
        before = Some(*progress.borrow());
    }
    if let Some(before) = before {
        wait_for_answer(progress, before).await;
    }
    Ok(())
}

/// Waits until an answer finishes after `before` was taken, at most [`SCRIPT_WAIT_MAX_MS`].
pub async fn wait_for_answer(progress: &mut watch::Receiver<Progress>, before: Progress) {
    wait_for(progress, |now| finished(before, now)).await;
}

fn finished(before: Progress, now: &Progress) -> bool {
    now.answers_finished > before.answers_finished
}

fn started(before: Progress, now: &Progress) -> bool {
    now.answers_started > before.answers_started
}

async fn wait_for(progress: &mut watch::Receiver<Progress>, done: impl Fn(&Progress) -> bool) {
    let limit = Duration::from_millis(SCRIPT_WAIT_MAX_MS);
    let waited = tokio::time::timeout(limit, progress.wait_for(|now| done(now))).await;
    match waited {
        Ok(Ok(_)) => {}
        Ok(Err(_)) => tracing::warn!("the session ended while the script was waiting"),
        Err(_) => tracing::warn!("script: waited {SCRIPT_WAIT_MAX_MS} ms; saying the next line"),
    }
}

#[cfg(test)]
mod tests {
    use super::{Cue, Script, ScriptError};

    #[test]
    fn lines_cues_comments_and_errors() {
        let script = Script::parse(
            "# a demo\n\nWalk me through it.\n  @4000   wait, why Redis first?  \nThanks.\n",
        );
        let Ok(script) = script else {
            panic!("parses");
        };
        let cues: Vec<(Cue, &str)> = script
            .lines
            .iter()
            .map(|line| (line.cue, line.text.as_str()))
            .collect();
        assert_eq!(
            cues,
            vec![
                (Cue::WhenIdle, "Walk me through it."),
                (Cue::IntoSpeech { delay_ms: 4000 }, "wait, why Redis first?"),
                (Cue::WhenIdle, "Thanks."),
            ]
        );
        assert_eq!(
            Script::parse("@soon hi"),
            Err(ScriptError::BadDelay { line: 1 })
        );
        assert_eq!(
            Script::parse("ok\n@300"),
            Err(ScriptError::BadDelay { line: 2 })
        );
        assert_eq!(
            Script::parse("@300   "),
            Err(ScriptError::BadDelay { line: 1 })
        );
    }
}
