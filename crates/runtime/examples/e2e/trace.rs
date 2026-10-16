//! What the run records: every input the core stepped, every UI event and latency mark, with
//! session times (the session's store sees them all), and the UI batches as the replayable
//! `events.jsonl` the UI's `?replay=` plays.

use std::fs::File;
use std::io::{BufWriter, Write as _};
use std::path::Path;
use std::sync::Mutex;

use boardai_core::effect::{ForgetScope, LogRecord};
use boardai_core::ids::CaptureId;
use boardai_core::input::Input;
use boardai_core::memory::MemoryWrite;
use boardai_core::protocol::{UiBatch, UiEvent, UiLogKind, UiLogLine};
use boardai_core::time::SessionTime;
use boardai_core::tools::ToolOutput;
use boardai_runtime::bridge::{UiSink, UiSinkClosed};
use boardai_runtime::session::{MemoryStore, SessionClose, SessionOpened, StoreRead, StoreSink};
use futures_util::future::BoxFuture;
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "k", rename_all = "snake_case")]
pub enum Entry {
    Input { input: Input },
    Ui { event: UiEvent },
    Record { record: LogRecord },
}

#[derive(Serialize)]
struct Line<'a> {
    t: SessionTime,
    #[serde(flatten)]
    entry: &'a Entry,
}

/// The session's store: keeps everything in order, and the memories `recall` needs.
#[derive(Default)]
pub struct Recorder {
    entries: Mutex<Vec<(SessionTime, Entry)>>,
    memory: MemoryStore,
}

impl Recorder {
    fn push(&self, at: SessionTime, entry: Entry) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.push((at, entry));
        }
    }

    pub fn entries(&self) -> Vec<(SessionTime, Entry)> {
        self.entries
            .lock()
            .map(|entries| entries.clone())
            .unwrap_or_default()
    }

    /// Writes the trace as JSON lines (`t`, `k` and the entry), for reading a run afterwards.
    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        let mut file = BufWriter::new(File::create(path)?);
        for (t, entry) in self.entries() {
            let line = serde_json::to_string(&Line { t, entry: &entry })?;
            writeln!(file, "{line}")?;
        }
        file.flush()
    }
}

impl StoreSink for Recorder {
    fn input(&self, at: SessionTime, input: &Input) {
        self.push(
            at,
            Entry::Input {
                input: input.clone(),
            },
        );
    }

    fn ui_event(&self, at: SessionTime, event: &UiEvent) {
        self.push(
            at,
            Entry::Ui {
                event: event.clone(),
            },
        );
    }

    fn log(&self, at: SessionTime, record: &LogRecord) {
        self.push(
            at,
            Entry::Record {
                record: record.clone(),
            },
        );
    }

    fn remember(&self, at: SessionTime, memory: &MemoryWrite) {
        self.memory.remember(at, memory);
    }

    fn forget(&self, at: SessionTime, scope: ForgetScope) {
        self.memory.forget(at, scope);
    }

    fn accept_proposal(&self, at: SessionTime, capture: CaptureId) {
        self.memory.accept_proposal(at, capture);
    }

    fn read(&self, read: StoreRead) -> BoxFuture<'static, ToolOutput> {
        self.memory.read(read)
    }

    fn start_session(&self) -> BoxFuture<'static, SessionOpened> {
        self.memory.start_session()
    }

    fn finish_session(&self, close: SessionClose) -> BoxFuture<'static, ()> {
        self.memory.finish_session(close)
    }

    fn flush(&self) -> BoxFuture<'static, ()> {
        Box::pin(std::future::ready(()))
    }
}

/// The UI's side: each batch as `events.jsonl` lines, exactly what the app's UI would get.
pub struct UiLog {
    file: Mutex<BufWriter<File>>,
}

impl UiLog {
    pub fn create(path: &Path) -> Result<Self, String> {
        let file = File::create(path).map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(Self {
            file: Mutex::new(BufWriter::new(file)),
        })
    }

    pub fn flush(&self) {
        if let Ok(mut file) = self.file.lock() {
            let _ = file.flush();
        }
    }
}

impl UiSink for UiLog {
    fn send(&self, batch: UiBatch) -> Result<(), UiSinkClosed> {
        let Ok(mut file) = self.file.lock() else {
            return Err(UiSinkClosed);
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
