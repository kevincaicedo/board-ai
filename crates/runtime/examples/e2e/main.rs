//! `just e2e`: the voice steel thread with real audio and no human (LLD §7). Lines spoken by
//! macOS `say` play into the mic ring in real time through a file mic, and everything after is
//! the app's own pipeline: Silero VAD and Whisper, the core, the talker (Ollama, gemma4 by
//! default), speech out (the system voice or Kokoro), the playback clock, ops.
//!
//! The voice plays through a null output (real time, nothing heard) unless `--output speakers`.
//! The run records `events.jsonl` (replay it in the UI with `?replay=`) and `trace.jsonl`, then
//! checks what happened and prints the latencies against HLD §8. It exits non-zero when a
//! check fails.
#![allow(clippy::print_stdout, reason = "a dev harness prints its report")]

mod check;
mod clips;
mod drive;
mod latency;
mod replay;
mod trace;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use boardai_runtime::audio::run_main_loop_until;

const USAGE: &str = "\
usage: just e2e [options]

  --script <file>     lines to say (the converse script format: `@<ms>` times a barge-in);
                      default: a question, a barge-in 3 s into the answer, then \"Okay, go on.\"
  --out <folder>      where clips, events.jsonl and trace.jsonl go; default target/e2e
  --voice system|kokoro   BoardAI's voice; default system
  --output null|speakers  default null (real time, silent)
  --model <name>      the Ollama model; default gemma4:latest
  --say-voice <name>  the macOS voice speaking the user's lines; default the system's
  --vocabulary <a,b>  terms Whisper should expect, as the app gives it the repo's symbols
  --repo <folder>     take Whisper's vocabulary from this repo's symbols, as the app does
  --strict            also fail on what depends on the model (opening a detour, resuming)";

const DEFAULT_SCRIPT: &str = "\
How does a request get from the browser to the database?
@3000 Wait, why Redis first?
Okay, go on.
";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Voice {
    System,
    Kokoro,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    Null,
    Speakers,
}

#[derive(Clone, Debug)]
pub struct Args {
    pub script: String,
    pub out: PathBuf,
    pub voice: Voice,
    pub output: Output,
    pub model: String,
    pub say_voice: Option<String>,
    pub vocabulary: Vec<String>,
    pub repo: Option<PathBuf>,
    pub strict: bool,
}

fn args() -> Result<Args, String> {
    let mut args = Args {
        script: DEFAULT_SCRIPT.to_owned(),
        out: PathBuf::from("target/e2e"),
        voice: Voice::System,
        output: Output::Null,
        model: "gemma4:latest".to_owned(),
        say_voice: None,
        vocabulary: Vec::new(),
        repo: None,
        strict: false,
    };
    let mut words = std::env::args().skip(1);
    while let Some(flag) = words.next() {
        if flag == "--strict" {
            args.strict = true;
            continue;
        }
        let value = words
            .next()
            .ok_or_else(|| format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--script" => {
                args.script =
                    std::fs::read_to_string(&value).map_err(|e| format!("{value}: {e}"))?;
            }
            "--out" => args.out = PathBuf::from(value),
            "--voice" => {
                args.voice = match value.as_str() {
                    "system" => Voice::System,
                    "kokoro" => Voice::Kokoro,
                    other => return Err(format!("unknown voice {other}")),
                }
            }
            "--output" => {
                args.output = match value.as_str() {
                    "null" => Output::Null,
                    "speakers" => Output::Speakers,
                    other => return Err(format!("unknown output {other}")),
                }
            }
            "--model" => args.model = value,
            "--say-voice" => args.say_voice = Some(value),
            "--vocabulary" => {
                args.vocabulary = value
                    .split(',')
                    .map(|term| term.trim().to_owned())
                    .collect();
            }
            "--repo" => args.repo = Some(PathBuf::from(value)),
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(args)
}

/// Marks the run finished when dropped.
struct Done(Arc<AtomicBool>);

impl Drop for Done {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

/// The system voice answers on the main thread's run loop, so the run happens on a worker
/// thread while `main` turns the loop.
fn main() -> ExitCode {
    if std::env::args().any(|arg| arg == "--help" || arg == "-h") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let args = match args() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("e2e: {error}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    if boardai_runtime::logging::init_stderr(tracing::Level::WARN).is_err() {
        eprintln!("e2e: logging was already set up");
    }
    let done = Arc::new(AtomicBool::new(false));
    let worker_done = Done(Arc::clone(&done));
    let worker = std::thread::spawn(move || {
        // Set however the run ends, a panic included, so `main` never waits forever.
        let _done = worker_done;
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())
            .and_then(|runtime| runtime.block_on(drive::run(&args)))
    });
    run_main_loop_until(|| done.load(Ordering::Acquire));
    match worker.join() {
        Ok(Ok(true)) => ExitCode::SUCCESS,
        Ok(Ok(false)) => ExitCode::FAILURE,
        Ok(Err(error)) => {
            eprintln!("e2e: {error}");
            ExitCode::FAILURE
        }
        Err(_) => {
            eprintln!("e2e: the run panicked");
            ExitCode::FAILURE
        }
    }
}
