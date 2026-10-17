//! The reader against gemma4's recorded streams (`tests/fixtures/ollama_chat/native_*.ndjson`,
//! gemma4:latest on Ollama 0.40.5; the unknown, malformed and unclosed ones are cut from
//! `native_call.ndjson`), at every split point, and against its bounds.

use std::sync::Arc;

use serde_json::{Map, Value, json};

use super::{NativeCall, NativeCalls};
use crate::limits::{OLLAMA_NATIVE_CALL_MAX_BYTES, OLLAMA_NATIVE_CALLS_PER_RESPONSE_MAX};

const TOOLS: &[&str] = &[
    "read_file",
    "search_code",
    "list_dir",
    "research",
    "review",
    "propose_change",
    "remember",
    "add_task",
    "recall",
    "open_session",
    "show_library",
    "show_tasks",
    "show_memory",
    "play_session",
];

const FIXTURES: &[(&str, &str)] = &[
    (
        "native_call",
        include_str!("../../../../tests/fixtures/ollama_chat/native_call.ndjson"),
    ),
    (
        "native_calls_two",
        include_str!("../../../../tests/fixtures/ollama_chat/native_calls_two.ndjson"),
    ),
    (
        "native_call_prose",
        include_str!("../../../../tests/fixtures/ollama_chat/native_call_prose.ndjson"),
    ),
    (
        "native_call_tag",
        include_str!("../../../../tests/fixtures/ollama_chat/native_call_tag.ndjson"),
    ),
    (
        "native_call_unknown",
        include_str!("../../../../tests/fixtures/ollama_chat/native_call_unknown.ndjson"),
    ),
    (
        "native_call_malformed",
        include_str!("../../../../tests/fixtures/ollama_chat/native_call_malformed.ndjson"),
    ),
    (
        "native_call_unclosed",
        include_str!("../../../../tests/fixtures/ollama_chat/native_call_unclosed.ndjson"),
    ),
];

#[derive(Debug, PartialEq)]
struct Outcome {
    text: String,
    calls: Vec<NativeCall>,
    read: usize,
    dropped: u32,
}

fn reader() -> NativeCalls {
    NativeCalls::new(TOOLS.iter().map(|name| (*name).to_owned()).collect())
}

fn read_pieces<'a>(pieces: impl IntoIterator<Item = &'a str>) -> Outcome {
    let mut reader = reader();
    let mut text = String::new();
    let mut calls = Vec::new();
    for piece in pieces {
        reader.push(piece, &mut text, &mut calls);
    }
    reader.finish(&mut text);
    Outcome {
        text,
        calls,
        read: reader.read(),
        dropped: reader.dropped(),
    }
}

fn read(text: &str) -> Outcome {
    read_pieces([text])
}

/// The streamed text of a recorded response, one piece per line as Ollama sent it.
fn contents(ndjson: &str) -> Vec<String> {
    ndjson
        .lines()
        .filter_map(|line| {
            let line: Value = serde_json::from_str(line).ok()?;
            Some(line["message"]["content"].as_str()?.to_owned())
        })
        .collect()
}

fn fixture(name: &str) -> Vec<String> {
    FIXTURES
        .iter()
        .find(|(fixture, _)| *fixture == name)
        .map(|(_, ndjson)| contents(ndjson))
        .unwrap_or_default()
}

fn call(name: &str, arguments: Value) -> NativeCall {
    let Value::Object(arguments) = arguments else {
        return NativeCall {
            name: name.to_owned(),
            arguments: Map::new(),
        };
    };
    NativeCall {
        name: name.to_owned(),
        arguments,
    }
}

const SAID: &str = "I see. I will call list_dir again.\n\n";

#[test]
fn a_call_written_in_gemmas_syntax_is_read_and_taken_out_of_the_text() {
    let outcome = read_pieces(fixture("native_call").iter().map(String::as_str));
    assert_eq!(outcome.text, SAID, "prose naming list_dir stays text");
    assert_eq!(outcome.calls, [call("list_dir", json!({"path": "."}))]);
    assert_eq!((outcome.read, outcome.dropped), (1, 0));
}

#[test]
fn two_calls_are_read_in_order() {
    let outcome = read_pieces(fixture("native_calls_two").iter().map(String::as_str));
    assert_eq!(
        outcome.text,
        "I will list the contents of the crates folder and the docs folder.\n\n\n"
    );
    assert_eq!(
        outcome.calls,
        [
            call("list_dir", json!({"path": "crates"})),
            call("list_dir", json!({"path": "docs"})),
        ]
    );
}

#[test]
fn prose_tags_and_unknown_tools_stay_text() {
    for name in [
        "native_call_prose",
        "native_call_tag",
        "native_call_unknown",
    ] {
        let pieces = fixture(name);
        let outcome = read_pieces(pieces.iter().map(String::as_str));
        assert_eq!(outcome.text, pieces.concat(), "{name}");
        assert_eq!((outcome.read, outcome.dropped), (0, 0), "{name}");
    }
}

#[test]
fn a_malformed_or_unclosed_call_is_dropped_and_counted() {
    for name in ["native_call_malformed", "native_call_unclosed"] {
        let outcome = read_pieces(fixture(name).iter().map(String::as_str));
        assert_eq!(outcome.text, SAID, "{name}");
        assert!(outcome.calls.is_empty(), "{name}");
        assert_eq!((outcome.read, outcome.dropped), (0, 1), "{name}");
    }
}

/// Every recorded response gives the same result fed whole, a line at a time as Ollama sent
/// it, cut in two at every character, and a character at a time.
#[test]
fn every_split_gives_the_same_result() {
    for (name, ndjson) in FIXTURES {
        let pieces = contents(ndjson);
        let whole = pieces.concat();
        let expected = read(&whole);
        assert_eq!(
            read_pieces(pieces.iter().map(String::as_str)),
            expected,
            "{name}"
        );
        for (index, _) in whole.char_indices().skip(1) {
            let (front, back) = whole.split_at(index);
            assert_eq!(read_pieces([front, back]), expected, "{name} at {index}");
        }
        let characters: Vec<String> = whole.chars().map(String::from).collect();
        assert_eq!(
            read_pieces(characters.iter().map(String::as_str)),
            expected,
            "{name} by character"
        );
    }
}

#[test]
fn gemmas_marks_decide_what_is_a_call() {
    let outcome = read("call:show_library{} and ");
    assert_eq!(outcome.calls, [call("show_library", json!({}))]);
    assert_eq!(outcome.text, " and ");
    let outcome = read("call:list_dir{path:\".\"}");
    assert_eq!(
        (outcome.read, outcome.dropped),
        (0, 1),
        "a JSON quote isn't gemma's"
    );
    assert_eq!(outcome.text, "");
    for words in [
        "show_library{} is words",
        "list_dir{path:\".\"}",
        "list_dir{path:'.'}",
        "xlist_dir{path:<|\"|>.<|\"|>}",
        "<list_dir{path:<|\"|>.<|\"|>}/>",
        "list_dir {path:<|\"|>.<|\"|>}",
        "tool_code:list_dir{path:<|\"|>.<|\"|>}",
    ] {
        let outcome = read(words);
        assert_eq!(outcome.text, words);
        assert_eq!((outcome.read, outcome.dropped), (0, 0), "{words}");
    }
}

#[test]
fn arguments_keep_their_types_and_a_closing_token_goes_with_the_call() {
    let outcome = read(
        "Reading it.\nread_file{end_line:30,path:<|\"|>src/main.rs<|\"|>,start_line:18}\
         <tool_call|>call:remember{about:<|\"|>you<|\"|>,kind:<|\"|>note<|\"|>,links:[],\
         text:<|\"|>Prefers {braces}, and \"quotes\"<|\"|>}<tool_call|> Done.",
    );
    assert_eq!(outcome.text, "Reading it.\n Done.");
    assert_eq!(
        outcome.calls,
        [
            call(
                "read_file",
                json!({"path": "src/main.rs", "start_line": 18, "end_line": 30})
            ),
            call(
                "remember",
                json!({"about": "you", "kind": "note", "links": [], "text": "Prefers {braces}, and \"quotes\""})
            ),
        ]
    );
}

#[test]
fn a_line_end_ends_an_open_call() {
    let outcome = read("list_dir{path:<|\"|>.<|\"|>\nNext, the crates.");
    assert_eq!(outcome.text, "\nNext, the crates.");
    assert_eq!((outcome.read, outcome.dropped), (0, 1));
    let outcome = read("list_dir{path:\".\"\nNext.");
    assert_eq!(outcome.text, "list_dir{path:\".\"\nNext.");
}

/// Past `OLLAMA_NATIVE_CALL_MAX_BYTES` the call is given up: dropped once it showed gemma's
/// syntax, given back otherwise; the rest is text again.
#[test]
fn an_open_call_is_bounded() {
    let long = "a".repeat(OLLAMA_NATIVE_CALL_MAX_BYTES);
    let marked = format!("recall{{query:<|\"|>{long}<|\"|>}} after");
    let outcome = read(&marked);
    assert_eq!((outcome.read, outcome.dropped), (0, 1));
    assert!(
        outcome.text.ends_with("<|\"|>} after"),
        "{}",
        &outcome.text[..40]
    );
    assert!(outcome.text.len() < marked.len());
    let unmarked = format!("recall{{query:{long}}} after");
    let outcome = read(&unmarked);
    assert_eq!(outcome.text, unmarked);
    assert_eq!((outcome.read, outcome.dropped), (0, 0));
    let frame = "recall{query:<|\"|><|\"|>}".len();
    let fits = format!(
        "recall{{query:<|\"|>{}<|\"|>}}",
        "a".repeat(OLLAMA_NATIVE_CALL_MAX_BYTES - frame)
    );
    assert_eq!(fits.len(), OLLAMA_NATIVE_CALL_MAX_BYTES);
    assert_eq!(read(&fits).read, 1, "a call of exactly the limit is read");
}

#[test]
fn calls_per_response_are_bounded() {
    let one = "list_dir{path:<|\"|>.<|\"|>}\n";
    let outcome = read(&one.repeat(OLLAMA_NATIVE_CALLS_PER_RESPONSE_MAX + 2));
    assert_eq!(outcome.read, OLLAMA_NATIVE_CALLS_PER_RESPONSE_MAX);
    assert_eq!(outcome.calls.len(), OLLAMA_NATIVE_CALLS_PER_RESPONSE_MAX);
    assert_eq!(outcome.dropped, 2);
    assert_eq!(
        outcome.text,
        "\n".repeat(OLLAMA_NATIVE_CALLS_PER_RESPONSE_MAX + 2)
    );
}

#[test]
fn a_reset_forgets_a_failed_attempt() {
    let mut reader = reader();
    let (mut text, mut calls) = (String::new(), Vec::new());
    reader.push("list_dir{path:<|\"|>", &mut text, &mut calls);
    reader.reset();
    reader.push("Hello.", &mut text, &mut calls);
    reader.finish(&mut text);
    assert_eq!(text, "Hello.");
    assert_eq!((reader.read(), reader.dropped()), (0, 0));
    let empty = NativeCalls::new(Arc::from(Vec::<String>::new()));
    assert_eq!(empty.read(), 0);
}
