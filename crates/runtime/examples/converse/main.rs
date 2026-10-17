//! `just converse`: the text-only dev loop (LLD §11 step 4). Typed lines are finished
//! utterances, the real talker answers with BoardAI's real system prompt and tools, and the
//! virtual voice "speaks" in real time, so ops fire in sync with captions exactly as in the app.
//! It prints what happens with session-time stamps, and the latency numbers per answer.

mod args;
mod print;

use std::fs::File;
use std::io::{BufWriter, Write as _};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use boardai_core::history::Conversation;
use boardai_core::protocol::{UiBatch, UiLogKind, UiLogLine};
use boardai_core::{CoreConfig, MicMode};
use boardai_runtime::bridge::{UiSink, UiSinkClosed};
use boardai_runtime::cancel::CancelToken;
use boardai_runtime::clock::{MonotonicClock, TokioClock};
use boardai_runtime::llm::openrouter::ProviderSlug;
use boardai_runtime::llm::{ChatStream, LlmCapabilities, LlmProvider};
use boardai_runtime::session::{
    Observed, Progress, Script, SessionChannels, SessionParts, SpeechInLink, StoreSink,
    VirtualVoice, VirtualVoiceConfig, run_script, say_typed, talker_from_settings, talker_name,
    wait_for_answer,
};
use boardai_runtime::settings::{LlmSettings, MemorySecretStore};
use boardai_runtime::tools::{RepoRoots, RepoTools};
use tokio::io::AsyncBufReadExt;
use tokio::sync::watch;

use args::Args;
use print::{Answer, Printer};

/// The UI is the terminal here. With `--ui-log` the batches are also written as the
/// `events.jsonl` lines the UI's replay mode plays (`?replay=<url>`), to look at the board.
#[derive(Default)]
struct UiLog {
    file: Option<Mutex<BufWriter<File>>>,
}

impl UiLog {
    fn create(path: Option<&std::path::Path>) -> Result<Self, String> {
        let Some(path) = path else {
            return Ok(Self::default());
        };
        let file = File::create(path).map_err(|error| format!("--ui-log: {error}"))?;
        Ok(Self {
            file: Some(Mutex::new(BufWriter::new(file))),
        })
    }

    fn flush(&self) {
        if let Some(Ok(mut file)) = self.file.as_ref().map(Mutex::lock) {
            let _ = file.flush();
        }
    }
}

impl UiSink for UiLog {
    fn send(&self, batch: UiBatch) -> Result<(), UiSinkClosed> {
        let Some(Ok(mut file)) = self.file.as_ref().map(Mutex::lock) else {
            return Ok(());
        };
        for event in batch.events {
            let line = UiLogLine {
                v: 1,
                t: batch.at,
                k: UiLogKind::Ui,
                event,
            };
            if let Ok(json) = serde_json::to_string(&line) {
                let _ = writeln!(file, "{json}");
            }
        }
        Ok(())
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    if std::env::args().any(|arg| arg == "--help" || arg == "-h") {
        println!("{}", args::USAGE);
        return ExitCode::SUCCESS;
    }
    let args = match Args::parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("{error}\n\n{}", args::USAGE);
            return ExitCode::from(2);
        }
    };
    let level = if args.verbose {
        tracing::Level::DEBUG
    } else {
        tracing::Level::WARN
    };
    if boardai_runtime::logging::init_stderr(level).is_err() {
        eprintln!("converse: logging was already set up");
    }
    match run(&args).await {
        Ok(answers) => {
            print_summary(&answers);
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("converse: {error}");
            ExitCode::FAILURE
        }
    }
}

/// The talker the options describe, and its settings.
fn talker(args: &Args) -> Result<(LlmSettings, Arc<dyn LlmProvider>), String> {
    let (settings, secret) = args.talker()?;
    let mut secrets = MemorySecretStore::new();
    if let Some((name, value)) = secret {
        secrets.insert(name, value);
    }
    let clock = Arc::new(MonotonicClock::start());
    let talker =
        talker_from_settings(&settings, &secrets, clock).map_err(|error| error.to_string())?;
    if args.warm_up {
        Ok((settings, talker))
    } else {
        Ok((settings, Arc::new(NoWarmUp(talker))))
    }
}

async fn run(args: &Args) -> Result<Vec<Answer>, String> {
    let (settings, talker) = talker(args)?;
    let tools = match &args.repo {
        Some(repo) => RepoTools::new(
            RepoRoots::open(std::slice::from_ref(repo)).map_err(|error| error.message)?,
        ),
        None => RepoTools::default(),
    };
    let script = match &args.script {
        Some(path) => {
            let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
            Some(Script::parse(&text).map_err(|error| error.to_string())?)
        }
        None => None,
    };
    let route: Vec<&str> = settings.route.iter().map(ProviderSlug::as_str).collect();
    println!(
        "converse: {} via {:?} at effort {}{}{}; type a line (Ctrl-D to end)",
        settings.model,
        args.provider,
        settings.effort.as_str(),
        if route.is_empty() {
            String::new()
        } else {
            format!(", routed to {}", route.join(", "))
        },
        if args.warm_up { "" } else { ", no warm-up" },
    );
    let printer = Arc::new(Printer::new(args.verbose, args.raw));
    let ui_log = Arc::new(UiLog::create(args.ui_log.as_deref())?);
    let (store, mut progress) = Observed::new(Arc::clone(&printer));
    let channels = SessionChannels::new();
    let voice = VirtualVoice::spawn(
        channels.speech_out(),
        VirtualVoiceConfig::with_speed(args.speed),
    );
    let parts = SessionParts {
        core: CoreConfig {
            talker_name: talker_name(&settings),
            mic: MicMode::OpenMic,
            session_context: None,
            suggestion: None,
            repo_reads: tools.repo_reads(),
        },
        clock: TokioClock::start(),
        talker,
        tools,
        harness: None,
        speech: Box::new(voice),
        store: Arc::new(store) as Arc<dyn StoreSink>,
        ui: Arc::clone(&ui_log) as Arc<dyn UiSink>,
        sources: boardai_runtime::session::SourceParts::in_memory(std::path::PathBuf::from("/")),
    };
    let session = boardai_runtime::session::spawn(channels, parts);
    tokio::time::sleep(std::time::Duration::from_millis(args.first_after_ms)).await;
    let link = session.speech_in();
    let said = match script {
        Some(script) => run_script(script, &link, &mut progress).await,
        None => read_stdin(&link, &mut progress).await,
    };
    let stats = session.close().await;
    ui_log.flush();
    said.map_err(|error| error.to_string())?;
    println!(
        "converse: {} steps, {} stale talker inputs dropped",
        stats.steps, stats.stale_talker_inputs
    );
    Ok(printer.answers())
}

/// The talker without its session warm-up, to measure what the warm-up saves.
struct NoWarmUp(Arc<dyn LlmProvider>);

impl LlmProvider for NoWarmUp {
    fn capabilities(&self) -> LlmCapabilities {
        self.0.capabilities()
    }

    fn stream(&self, conversation: &Conversation, cancel: CancelToken) -> ChatStream {
        self.0.stream(conversation, cancel)
    }
}

/// Says each typed line at once; at the end waits for the last answer to finish.
async fn read_stdin(
    link: &SpeechInLink,
    progress: &mut watch::Receiver<Progress>,
) -> Result<(), boardai_runtime::session::SessionClosed> {
    let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    let mut before = None;
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        say_typed(link, &line).await?;
        before = Some(*progress.borrow());
    }
    if let Some(before) = before {
        wait_for_answer(progress, before).await;
    }
    Ok(())
}

fn print_summary(answers: &[Answer]) {
    println!(
        "\nanswer  first token  first audio  first op  ops  tags  dropped  tools  preamble  \
         missing  per word  markup  question"
    );
    for (index, answer) in answers.iter().enumerate() {
        let ms = |value: Option<u64>| value.map_or_else(|| "-".to_owned(), |ms| ms.to_string());
        let mut question = answer.question.clone();
        boardai_runtime::logging::bound(&mut question, 48);
        let quality = answer.quality();
        println!(
            "{:>6}  {:>11}  {:>11}  {:>8}  {:>3}  {:>4}  {:>7}  {:>5}  {:>8}  {:>7}  {:>8}  {:>6}  \
             {}{}",
            index + 1,
            ms(answer.first_token_ms),
            ms(answer.first_audio_ms),
            ms(answer.first_op_ms),
            answer.ops,
            answer.tags_written(),
            answer.dropped_tags,
            answer.tool_calls,
            quality.preamble_words,
            quality.missing_words,
            quality.drawing_tags_per_word_max,
            quality.markup,
            question,
            if answer.interrupted {
                " (interrupted)"
            } else {
                ""
            },
        );
    }
}
