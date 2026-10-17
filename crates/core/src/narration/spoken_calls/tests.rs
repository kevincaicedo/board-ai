use super::{
    READ_FILLERS, READ_OBJECTS, READ_OPENERS, READ_VERBS, SPOKEN_TOOLS, STRAY_TOKENS, SpokenCalls,
    TOOL_CODE_BLOCKS, tool_name, without_stray_tokens,
};
use crate::limits::SPOKEN_CALL_MAX_BYTES;
use crate::tools::TOOL_NAMES;

/// The table follows the schemas: every tool, every parameter, in order.
#[test]
fn the_table_matches_the_tool_schemas() {
    let names: Vec<&str> = SPOKEN_TOOLS.iter().map(|(name, _)| *name).collect();
    assert_eq!(names, TOOL_NAMES.to_vec());
    for schema in crate::tools::schemas() {
        let mut properties: Vec<&str> = schema.input_schema["properties"]
            .as_object()
            .map(|object| object.keys().map(String::as_str).collect())
            .unwrap_or_default();
        let mut table: Vec<&str> = SPOKEN_TOOLS
            .iter()
            .find(|(name, _)| *name == schema.name)
            .map(|(_, parameters)| parameters.to_vec())
            .unwrap_or_default();
        properties.sort_unstable();
        table.sort_unstable();
        assert_eq!(table, properties, "{}", schema.name);
    }
}

/// What feeding `text` word by word leaves: the speech, the calls kept out of it, and the
/// reads it announced.
struct Fed {
    said: String,
    calls: Vec<String>,
    announced: Vec<&'static str>,
}

fn feed(text: &str) -> Fed {
    let mut calls = SpokenCalls::default();
    let mut out = String::new();
    let mut word = String::new();
    for ch in text.chars().chain(std::iter::once('\n')) {
        if ch.is_whitespace() {
            let ended = without_stray_tokens(std::mem::take(&mut word));
            if !ended.is_empty() {
                let fate = calls.word(&ended);
                out.push_str(&fate.released);
                if fate.speak {
                    out.push_str(&ended);
                }
            }
            out.push_str(&calls.whitespace(ch));
        } else {
            word.push(ch);
        }
    }
    out.push_str(&calls.boundary());
    Fed {
        said: out.trim_end().to_owned(),
        calls: calls
            .calls()
            .iter()
            .map(|call| call.written.clone())
            .collect(),
        announced: calls.announced().to_vec(),
    }
}

fn speech(text: &str) -> (String, Vec<String>) {
    let fed = feed(text);
    (fed.said, fed.calls)
}

#[test]
fn calls_written_as_words_are_kept_out_of_speech() {
    for (text, spoken, call) in [
        (
            r#"Let me look. list_dir path "." Then I read."#,
            "Let me look. Then I read.",
            r#"list_dir path ".""#,
        ),
        (
            r#"Open read_file("src/a b.rs") now."#,
            "Open now.",
            r#"read_file("src/a b.rs")"#,
        ),
        (
            r#"`search_code query="TODO"` finds it."#,
            "finds it.",
            r#"`search_code query="TODO"`"#,
        ),
        (
            r#"Later research(question="why", depth="quick") helps."#,
            "Later helps.",
            r#"research(question="why", depth="quick")"#,
        ),
        (
            r#"list_dir {"path": "crates"} shows it."#,
            "shows it.",
            r#"list_dir {"path": "crates"}"#,
        ),
    ] {
        let (said, calls) = speech(text);
        assert_eq!(said, spoken, "{text}");
        assert_eq!(calls, vec![call.to_owned()], "{text}");
    }
}

#[test]
fn words_that_only_name_a_tool_stay_speech() {
    for text in [
        "Research shows caches help, and I recall the review.",
        r#"Please remember kind="note" there."#,
        "list_dir shows the folders.",
        "read_files are not tools.",
        "The list_dir",
    ] {
        let (said, calls) = speech(text);
        assert_eq!(said, text, "{text}");
        assert!(calls.is_empty(), "{text}");
    }
}

#[test]
fn a_call_ends_at_a_line_end_and_at_its_bound() {
    let (said, _) = speech("read_file(\"a\",\nstart_line=1) spans lines.");
    assert_eq!(said, "\nstart_line=1) spans lines.");
    let long = format!(
        "read_file(\"{}\" and more words",
        "x ".repeat(SPOKEN_CALL_MAX_BYTES)
    );
    let (said, calls) = speech(&long);
    assert!(
        said.ends_with("and more words") || said.contains("x x"),
        "{said}"
    );
    assert!(
        calls[0].len() <= SPOKEN_CALL_MAX_BYTES + 8,
        "{}",
        calls[0].len()
    );
    assert_eq!(tool_name("read_file_x"), None);
}

/// gemma4's forms on Ollama (T-026), each kept out of speech without running anything.
#[test]
fn gemmas_tool_code_forms_are_kept_out_of_speech() {
    for (text, spoken, calls) in [
        (
            "Okay. tool_code:list_dir{path:<|\"|>.<|\"|>} Then more.",
            "Okay. Then more.",
            vec!["tool_code:list_dir{path:<|\"|>.<|\"|>}"],
        ),
        (
            "call:read_file{path:<|\"|>src/a b.rs<|\"|>} next.",
            "next.",
            vec!["call:read_file{path:<|\"|>src/a b.rs<|\"|>}"],
        ),
        (
            "tool_code\nlist_dir{path:<|\"|>.<|\"|>}\nend_tool_code\nLet me see.",
            "Let me see.",
            vec!["list_dir{path:<|\"|>.<|\"|>}"],
        ),
        (
            "So.\n[tool_code]\nprint(list_dir(path=\".\"))\n[/tool_code]\nDone.",
            "So.\n\nDone.",
            vec!["print(list_dir(path=\".\"))"],
        ),
        (
            "tool_code\nlist_dir path \".\"\nread_file(\"a.rs\")\nend_tool_code Then.",
            "Then.",
            vec!["list_dir path \".\"", "read_file(\"a.rs\")"],
        ),
        // Ollama's adapter read the call out of the block (D-191): its markers stay out too.
        (
            "tool_code\n\nend_tool_code\nLet me see the top level.",
            "Let me see the top level.",
            vec![],
        ),
    ] {
        let fed = feed(text);
        assert_eq!(fed.said.trim_start(), spoken, "{text:?}");
        assert_eq!(fed.calls, calls, "{text:?}");
    }
}

/// The markers count only around a call: prose that names them, or a block with no call in
/// it, stays speech.
#[test]
fn tool_code_words_without_a_call_stay_speech() {
    for text in [
        "The tool_code field holds it.",
        "tool_code\nis a field.",
        "[tool_code] shows research first.",
        "end_tool_code alone",
        "tool_code:print(x) is not ours.",
    ] {
        let (said, calls) = speech(text);
        assert_eq!(said, text, "{text:?}");
        assert!(calls.is_empty(), "{text:?}");
    }
    for (opening, closing) in TOOL_CODE_BLOCKS {
        for closing in *closing {
            let text = format!("A {opening}\nlist_dir{{}}\n{closing} B");
            assert_eq!(speech(&text).0, "A  B", "{text:?}");
        }
    }
}

/// A block whose closing word never comes ends at the first word that is no call, or at the
/// bound; the speech after it is said.
#[test]
fn an_unclosed_block_ends_at_speech_or_its_bound() {
    let (said, calls) = speech("tool_code\nlist_dir{}\nand then speech.");
    assert_eq!((said.as_str(), calls.len()), ("and then speech.", 1));
    let text = format!(
        "tool_code\nlist_dir{{path:\"{}\"}} and then speech.",
        "x ".repeat(SPOKEN_CALL_MAX_BYTES)
    );
    let (said, calls) = speech(&text);
    assert!(
        said.ends_with("and then speech.") && said.contains("x x"),
        "{said}"
    );
    assert!(
        calls[0].len() <= SPOKEN_CALL_MAX_BYTES + 8,
        "{}",
        calls[0].len()
    );
}

#[test]
fn stray_channel_tokens_are_taken_out_of_their_word() {
    for token in STRAY_TOKENS {
        let text = format!("the repository structure.{token} Then{token}more. {token}");
        assert_eq!(
            speech(&text).0,
            "the repository structure. Thenmore.",
            "{token}"
        );
    }
    assert_eq!(without_stray_tokens("<|chan<channel|>nel>".to_owned()), "");
}

/// The measured first turns (T-026), and announcements in other words.
#[test]
fn announced_repo_reads_are_noted_with_the_tool_they_imply() {
    for (text, read) in [
        (
            "I will start by listing the root directory to see the structure.",
            "list_dir",
        ),
        (
            "Okay. I will start by listing the root directory to understand the repository \
             structure.<channel|>",
            "list_dir",
        ),
        (
            "I will start by listing the top level of the repository.",
            "list_dir",
        ),
        (
            "I will start by looking at the directory structure.",
            "list_dir",
        ),
        ("Let me look at the repo structure first.", "list_dir"),
        ("I’ll list the files.", "list_dir"),
        ("Let’s take a look at the code.", "list_dir"),
        ("Then I will examine key subdirectories.", "list_dir"),
        ("Let me read the README.md first.", "read_file"),
        ("I'm going to open crates/core/src/lib.rs now.", "read_file"),
        (
            "I will search the codebase for the entry point.",
            "search_code",
        ),
        ("I'll go ahead and search the codebase.", "search_code"),
    ] {
        assert_eq!(feed(text).announced, vec![read], "{text}");
    }
    let both = feed("I'll list the files, then I'll search the code for the parser.");
    assert_eq!(both.announced, vec!["list_dir", "search_code"]);
    let again = feed("I'll list the files. Then let me look at the folders.");
    assert_eq!(again.announced, vec!["list_dir"], "each tool once");
}

#[test]
fn other_words_announce_no_read() {
    for text in [
        "I'll draw it.",
        "I'll draw the repo structure.",
        "Let me explain.",
        "Let me explain the code structure.",
        "Let me explain the code.",
        "I'll list three reasons.",
        "Let me look. The files are big.",
        "I will. List the files later.",
        "Let me know if the repo looks right.",
        "You'll see the files soon.",
        "Do you want me to look at the contents of a specific file?",
        "I listed the files.",
        // An algorithm explained with a repo open.
        "Let's search for 7 in the tree.",
        "Let me look at the root of the tree.",
        "Let me check the entries in the table.",
        "Let's find the source vertex first.",
        "I'll look into how caching works and then we can talk about it for a while.",
    ] {
        assert!(feed(text).announced.is_empty(), "{text}");
    }
}

/// Every table entry takes part: each opener, filler, verb and object makes an announcement in
/// a sentence built around it.
#[test]
fn every_announcement_word_counts() {
    for opener in READ_OPENERS {
        assert_eq!(
            feed(&format!("{opener} list the files.")).announced,
            ["list_dir"]
        );
    }
    for filler in READ_FILLERS {
        let text = format!("I will {filler} list the files.");
        assert_eq!(feed(&text).announced, ["list_dir"], "{text}");
    }
    for (verb, tool) in READ_VERBS {
        assert_eq!(feed(&format!("Let me {verb} the repo.")).announced, [*tool]);
    }
    for object in READ_OBJECTS {
        let text = format!("Let me look at the {object}.");
        assert_eq!(feed(&text).announced, ["list_dir"], "{text}");
    }
}

/// The words a call keeps out of speech never announce anything, and a stream split anywhere
/// announces the same.
#[test]
fn announcements_come_from_speech_only() {
    assert!(
        feed("list_dir path \"I'll list the files\" ok")
            .announced
            .is_empty()
    );
    assert_eq!(
        feed("Let me tool_code\nlist_dir{}\nend_tool_code look at the files.").announced,
        ["list_dir"]
    );
}
