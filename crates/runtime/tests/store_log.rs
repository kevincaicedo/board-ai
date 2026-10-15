//! The session event log: the UI's replay format, input lines for re-simulation, flushing,
//! bounded lines, a full inbox, and a reader that skips and counts bad lines.
#![allow(clippy::expect_used, reason = "test setup fails loudly")]

mod store_support;
mod support;

use std::io::Cursor;
use std::path::Path;

use boardai_core::effect::{LogRecord, Milestone};
use boardai_core::ids::ToolCallId;
use boardai_core::input::Input;
use boardai_core::protocol::{MicAction, Speaker, UiCommand, UiEvent, UiLogLine};
use boardai_core::tools::ToolOutput;
use boardai_runtime::limits::{EVENT_LOG_FLUSH_MS, EVENT_LOG_LINE_MAX_BYTES};
use boardai_runtime::store::{
    DropReason, EventLogReader, LogLine, Logged, SessionEnded, SkipCounts, StoreConfig,
};
use store_support::{TestStore, at, caption, close, node};
use support::TempFolder;

const DEMO_FIXTURE: &str = "../../ui/public/fixtures/demo.events.jsonl";

fn read_lines(path: &Path) -> (Vec<LogLine>, SkipCounts) {
    let mut reader = EventLogReader::open(path).expect("log opens");
    let lines = reader
        .by_ref()
        .collect::<Result<Vec<_>, _>>()
        .expect("readable");
    (lines, reader.skipped())
}

fn session_log(test: &TestStore, folder: &str) -> std::path::PathBuf {
    test.vault().join(folder).join("events.jsonl")
}

#[tokio::test]
async fn ui_lines_are_written_exactly_as_the_ui_replays_them() {
    let fixture = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(DEMO_FIXTURE))
        .expect("the UI's demo fixture exists");
    let fixture_lines: Vec<&str> = fixture.lines().filter(|line| !line.is_empty()).collect();
    let (lines, skipped) = read_lines(&Path::new(env!("CARGO_MANIFEST_DIR")).join(DEMO_FIXTURE));
    assert_eq!(skipped, SkipCounts::default());
    assert_eq!(lines.len(), fixture_lines.len());

    let folder = TempFolder::new("store-log-ui");
    let test = TestStore::new(&folder.path);
    let (store, task, _) = test.open().await;
    let session = store.start_session(None).await.expect("starts");
    for line in &lines {
        let LogLine::Ui { t, event } = line else {
            panic!("the demo holds only ui lines");
        };
        assert_eq!(store.log_ui(*t, vec![event.clone()]), Logged::Accepted);
        store.flush().await.expect("flush");
    }
    let last = lines.last().map_or(at(0), LogLine::time);
    let ended = store.end_session(last).await.expect("ends");
    assert!(matches!(ended, SessionEnded::Material(_)));

    let written = std::fs::read_to_string(session_log(&test, &session.folder)).expect("log");
    let ui_lines: Vec<&str> = written
        .lines()
        .filter(|line| line.contains(r#""k":"ui""#))
        .collect();
    assert_eq!(
        ui_lines, fixture_lines,
        "byte-for-byte the UI's replay format"
    );
    let first: serde_json::Value =
        serde_json::from_str(written.lines().next().unwrap_or_default()).expect("json");
    assert_eq!(first["k"], "session");
    assert_eq!(first["v"], 1);
    let parsed: UiLogLine = serde_json::from_str(ui_lines[0]).expect("a UiLogLine");
    assert_eq!(parsed.v, 1);
    close(store, task).await;
}

#[tokio::test]
async fn inputs_and_records_round_trip_for_re_simulation() {
    let folder = TempFolder::new("store-log-inputs");
    let test = TestStore::new(&folder.path);
    let (store, task, _) = test.open().await;
    let session = store.start_session(None).await.expect("starts");
    let inputs = vec![
        Input::VoiceStart,
        Input::Transcript {
            text: "walk me through it".into(),
            is_final: true,
        },
        Input::Command(UiCommand::Mic {
            action: MicAction::Mute,
        }),
        Input::ToolResult {
            id: ToolCallId::parse("toolu_01").expect("id"),
            output: ToolOutput::Text {
                text: "Found in your vault".into(),
            },
        },
        Input::VoiceEnd { voiced_ms: 900 },
    ];
    for (index, input) in inputs.iter().enumerate() {
        store.log_input(at(index as u64 * 10), input.clone());
    }
    let record = LogRecord::Milestone {
        milestone: Milestone::FirstAudio,
    };
    store.log_record(at(60), record.clone());
    store.end_session(at(100)).await.expect("ends");

    let (lines, skipped) = read_lines(&session_log(&test, &session.folder));
    assert_eq!(skipped, SkipCounts::default());
    let read_inputs: Vec<Input> = lines
        .iter()
        .filter_map(|line| {
            if let LogLine::Input { input, .. } = line {
                Some(input.clone())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(read_inputs, inputs);
    assert!(lines.contains(&LogLine::Record { t: at(60), record }));
    assert!(
        matches!(lines.first(), Some(LogLine::Session { id, .. }) if id.starts_with("2026-10-15-"))
    );
    assert_eq!(lines.last(), Some(&LogLine::End { t: at(100) }));
    let raw = std::fs::read_to_string(session_log(&test, &session.folder)).expect("log");
    assert!(raw.contains(r#"{"v":1,"t":0,"k":"input","input":{"input":"voice_start"}}"#));
    close(store, task).await;
}

#[tokio::test]
async fn an_oversized_record_becomes_a_counted_marker_never_a_cut_line() {
    let folder = TempFolder::new("store-log-oversized");
    let test = TestStore::new(&folder.path);
    let (store, task, _) = test.open().await;
    let session = store.start_session(None).await.expect("starts");
    let huge = "x".repeat(EVENT_LOG_LINE_MAX_BYTES + 10);
    store.log_ui(at(5), vec![caption(Speaker::User, "before")]);
    store.log_input(
        at(6),
        Input::Transcript {
            text: huge,
            is_final: true,
        },
    );
    store.log_ui(at(7), vec![caption(Speaker::User, "after")]);
    store.end_session(at(8)).await.expect("ends");

    let raw = std::fs::read_to_string(session_log(&test, &session.folder)).expect("log");
    assert!(
        raw.lines()
            .all(|line| line.len() <= EVENT_LOG_LINE_MAX_BYTES)
    );
    let (lines, skipped) = read_lines(&session_log(&test, &session.folder));
    assert_eq!(skipped, SkipCounts::default(), "every line still parses");
    assert!(lines.contains(&LogLine::Dropped {
        t: at(6),
        reason: DropReason::Oversized,
        count: 1
    }));
    let captions = lines
        .iter()
        .filter(|line| {
            matches!(
                line,
                LogLine::Ui {
                    event: UiEvent::Caption { .. },
                    ..
                }
            )
        })
        .count();
    assert_eq!(captions, 2, "the lines around it are kept");
    close(store, task).await;
}

#[tokio::test]
async fn a_full_inbox_drops_and_counts_log_entries_and_never_blocks() {
    let folder = TempFolder::new("store-log-full");
    let test = TestStore::new(&folder.path);
    let config = StoreConfig {
        inbox_capacity: 4,
        ..test.config()
    };
    let (store, task, _) = boardai_runtime::store::Store::open(config)
        .await
        .expect("opens");
    let session = store.start_session(None).await.expect("starts");
    // On this single-threaded runtime the actor can't run until the test awaits, so the
    // inbox fills after four entries and the rest must be dropped, not waited for.
    let outcomes: Vec<Logged> = (0..10)
        .map(|n| store.log_ui(at(n), vec![caption(Speaker::User, "hello")]))
        .collect();
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == Logged::Accepted)
            .count(),
        4
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == Logged::Dropped)
            .count(),
        6
    );
    assert_eq!(store.dropped_log_entries(), 6);
    store.end_session(at(20)).await.expect("ends");
    let (lines, _) = read_lines(&session_log(&test, &session.folder));
    assert!(
        lines.iter().any(|line| matches!(
            line,
            LogLine::Dropped {
                reason: DropReason::InboxFull,
                count: 6,
                ..
            }
        )),
        "{lines:?}"
    );
    close(store, task).await;
}

#[tokio::test(start_paused = true)]
async fn entries_reach_the_file_within_one_flush_interval_without_a_flush_call() {
    let folder = TempFolder::new("store-log-interval");
    let test = TestStore::new(&folder.path);
    let (store, task, _) = test.open().await;
    let session = store.start_session(None).await.expect("starts");
    let log = session_log(&test, &session.folder);
    store.log_ui(at(1), vec![caption(Speaker::User, "flushed by the timer")]);
    let on_disk = || {
        std::fs::read_to_string(&log)
            .unwrap_or_default()
            .contains("flushed by the timer")
    };
    assert!(!on_disk(), "buffered, not written per entry");
    tokio::time::sleep(std::time::Duration::from_millis(EVENT_LOG_FLUSH_MS + 1)).await;
    // The flush itself runs on a blocking thread in real time.
    let mut waited = 0;
    while !on_disk() && waited < 200 {
        std::thread::sleep(std::time::Duration::from_millis(10));
        tokio::task::yield_now().await;
        waited += 1;
    }
    assert!(
        on_disk(),
        "a crash after one interval loses nothing written before it"
    );
    close(store, task).await;
}

#[test]
fn the_reader_skips_and_counts_bad_lines() {
    let long = format!(
        r#"{{"v":1,"t":1,"k":"ui","event":{{"type":"Caption","who":"User","text":"{}","is_final":true}}}}"#,
        "y".repeat(EVENT_LOG_LINE_MAX_BYTES)
    );
    let text = [
        r#"{"v":1,"t":0,"k":"session","id":"2026-10-15-143210","started":"2026-10-15T14:32:10Z"}"#,
        "",
        "not json",
        r#"{"v":2,"t":5,"k":"ui","event":{"type":"Acknowledge"}}"#,
        r#"{"v":1,"t":5,"k":"telepathy"}"#,
        r#"{"v":1,"t":6,"k":"ui","event":{"type":"Acknowledge"}}"#,
        long.as_str(),
        r#"{"v":1,"t":7,"k":"ui","event":{"type":"Nope"}}"#,
        r#"{"v":1,"t":9,"k":"end"}"#,
    ]
    .join("\n");
    let mut reader = EventLogReader::new(Cursor::new(text.into_bytes()));
    let lines: Vec<LogLine> = reader
        .by_ref()
        .map(|line| line.expect("readable"))
        .collect();
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert_eq!(
        lines[1],
        LogLine::Ui {
            t: at(6),
            event: UiEvent::Acknowledge
        }
    );
    assert_eq!(
        reader.skipped(),
        SkipCounts {
            malformed: 3,
            oversized: 1,
            unsupported_version: 1
        }
    );
}

#[tokio::test]
async fn nothing_is_logged_outside_a_session() {
    let folder = TempFolder::new("store-log-outside");
    let test = TestStore::new(&folder.path);
    let (store, task, _) = test.open().await;
    store.log_ui(at(1), vec![node(0, "lb", "Load balancer")]);
    store.flush().await.expect("flush");
    let session = store.start_session(None).await.expect("starts");
    store.end_session(at(2)).await.expect("ends");
    let (lines, _) = read_lines(&session_log(&test, &session.folder));
    assert!(!lines.iter().any(|line| matches!(line, LogLine::Ui { .. })));
    close(store, task).await;
}
