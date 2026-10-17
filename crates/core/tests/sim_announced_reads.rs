//! Simulator scenarios for a repo read announced and not made (D-232): gemma4's first turn said
//! "I will start by listing the root directory" and ended without a call in 16 to 18 of 20 repo
//! tours (T-026). The core asks once more for the call, naming the tool the words imply, only
//! when the session offers the repo reads over an open repo and the stream called no tool;
//! within `NUDGES_PER_TURN_MAX` and behind every gate of D-178.
#![allow(clippy::expect_used, reason = "a broken scenario should fail loudly")]

mod common;

use boardai_core::conversation::ConversationState;
use boardai_core::effect::Effect;
use boardai_core::input::{Input, StopReason};
use boardai_core::protocol::{DrawAction, UiCommand};
use boardai_core::sim::Sim;
use boardai_core::sim::talker::ScriptedReply;
use boardai_core::tools::{DirEntry, ToolOutput};
use boardai_core::{CoreConfig, RepoReads};
use common::{assert_clean, config, count, op_starts};

/// gemma4's first turn, as measured.
const ANNOUNCES: &str = "I will start by listing the root directory to see the structure.";

const DRAWS_CRATES: &str =
    r#"The code lives in <node id="crates" label="crates/" kind="group"/>crates."#;

/// What the note asks when a read was announced.
const ASKED: &str = "said it would look at the repo but called no tool";

fn repo_open() -> CoreConfig {
    CoreConfig {
        repo_reads: RepoReads::Offered,
        ..config()
    }
}

fn talks(sim: &Sim) -> usize {
    count(sim, |effect| matches!(effect, Effect::Talk { .. }))
}

fn ran(sim: &Sim) -> usize {
    count(sim, |effect| matches!(effect, Effect::RunTool { .. }))
}

/// Every message of request `index`, as text.
fn request_text(sim: &Sim, index: usize) -> String {
    format!(
        "{:?}",
        sim.talker.requests.get(index).expect("a request").messages
    )
}

fn asked_in_any_request(sim: &Sim) -> bool {
    (0..sim.talker.requests.len()).any(|index| request_text(sim, index).contains(ASKED))
}

fn tour(config: CoreConfig) -> Sim {
    let mut sim = Sim::new(config);
    sim.tools.respond(
        "list_dir",
        ToolOutput::Listing {
            entries: vec![DirEntry {
                name: "crates".to_owned(),
                is_dir: true,
            }],
            more_entries: 0,
        },
    );
    sim
}

#[test]
fn an_announced_read_is_asked_for_as_a_call() {
    let mut sim = tour(repo_open());
    sim.talker.push(ScriptedReply::says(ANNOUNCES));
    sim.talker
        .push(ScriptedReply::says("Listing it now.").calls("list_dir", r#"{"path":"."}"#));
    sim.talker.push(ScriptedReply::says(DRAWS_CRATES));
    sim.user_says(0, "Walk me through this repo");
    sim.run_until(40_000);
    assert_clean(&sim);
    assert_eq!(talks(&sim), 3, "asked once, then the tool round");
    let asked = request_text(&sim, 1);
    assert!(
        asked.contains("Call list_dir now as a tool call"),
        "{asked}"
    );
    assert!(
        asked.contains(ANNOUNCES),
        "what it said stays in the history: {asked}"
    );
    assert_eq!(ran(&sim), 1);
    assert_eq!(op_starts(&sim).len(), 1, "it drew what it read");
    let said = sim.effects().any(|(_, effect)| {
        matches!(effect, Effect::Synthesize { text, .. } if text.contains("listing the root"))
    });
    assert!(said, "the announcement itself was speech");
}

/// The note names the tool the words imply.
#[test]
fn the_note_names_the_announced_tool() {
    for (announces, tool) in [
        ("I'll search the codebase for the parser.", "search_code"),
        ("Let me read the README.md first.", "read_file"),
        ("Let me take a look at the repo structure.", "list_dir"),
    ] {
        let mut sim = tour(repo_open());
        sim.talker.push(ScriptedReply::says(announces));
        sim.talker.push(ScriptedReply::says("Unused."));
        sim.user_says(0, "Walk me through this repo");
        sim.run_until(40_000);
        assert_clean(&sim);
        assert_eq!(talks(&sim), 2, "{announces}");
        let asked = request_text(&sim, 1);
        assert!(asked.contains(&format!("Call {tool} now")), "{asked}");
    }
}

/// Without an open repo there was nothing to call: the reply ends as it is.
#[test]
fn no_retry_when_no_repo_is_open() {
    let mut sim = tour(config());
    sim.talker.push(ScriptedReply::says(ANNOUNCES));
    sim.talker.push(ScriptedReply::says("Unused."));
    sim.user_says(0, "Walk me through this repo");
    sim.run_until(40_000);
    assert_clean(&sim);
    assert_eq!(talks(&sim), 1);
}

/// "I'll draw it" and "let me explain" announce no read; an announced read the stream made is
/// not asked for again.
#[test]
fn no_retry_for_other_words_or_a_read_that_was_made() {
    let mut sim = tour(repo_open());
    sim.talker.push(ScriptedReply::says(
        "I'll draw it in a moment, but let me explain first: a repo holds crates.",
    ));
    sim.user_says(0, "What is a repo?");
    sim.run_until(20_000);
    assert_eq!(talks(&sim), 1, "no read announced");
    sim.talker
        .push(ScriptedReply::says("Let me explain the code as I go."));
    sim.user_says(20_000, "And then?");
    sim.run_until(40_000);
    assert_eq!(talks(&sim), 2, "no read announced");
    sim.talker
        .push(ScriptedReply::says("Let me list the files.").calls("list_dir", r#"{"path":"."}"#));
    sim.talker.push(ScriptedReply::says(DRAWS_CRATES));
    sim.user_says(40_000, "Walk me through this repo");
    sim.run_until(80_000);
    assert_eq!(talks(&sim), 4, "the tool round only");
    // A stream that called and still said it ended its turn made its read all the same.
    sim.talker.push(
        ScriptedReply::says("Let me read the README.md now.")
            .calls("read_file", r#"{"path":"README.md"}"#)
            .stops(StopReason::EndTurn),
    );
    sim.talker.push(ScriptedReply::says("Unused."));
    sim.user_says(80_000, "And the readme?");
    sim.run_until(120_000);
    assert_clean(&sim);
    assert_eq!(talks(&sim), 5);
    assert!(!asked_in_any_request(&sim));
}

/// The bound: a retry that announces again ends the turn; the next turn may ask once more.
#[test]
fn an_announced_read_is_asked_for_once_a_turn() {
    let mut sim = tour(repo_open());
    sim.talker.push(ScriptedReply::says(ANNOUNCES));
    sim.talker
        .push(ScriptedReply::says("Sure, I'll list the files."));
    sim.user_says(0, "Walk me through this repo");
    sim.run_until(40_000);
    assert_eq!(talks(&sim), 2);
    assert_eq!(ran(&sim), 0);
    sim.talker.push(ScriptedReply::says(ANNOUNCES));
    sim.talker.push(ScriptedReply::says("Unused."));
    sim.user_says(40_000, "Walk me through this repo");
    sim.run_until(80_000);
    assert_clean(&sim);
    assert_eq!(talks(&sim), 4, "a new turn may ask once more");
}

/// A stream that announced a read and made none was touring all the same: the module it names
/// without having read anything is drawn marked "?" (D-180).
#[test]
fn a_tour_that_only_announced_marks_what_it_never_read() {
    let mut sim = tour(repo_open());
    sim.talker.push(ScriptedReply::says(concat!(
        "I will start by listing the root directory. Then the ",
        r#"<node id="engine" label="engine"/>engine runs it."#,
    )));
    sim.talker.push(ScriptedReply::says("Unused."));
    sim.user_says(0, "Walk me through this repo");
    sim.run_until(40_000);
    assert_clean(&sim);
    let marked = op_starts(&sim)
        .iter()
        .any(|(_, event)| format!("{event:?}").contains("label: \"engine ?\""));
    assert!(marked, "{:?}", op_starts(&sim));
}

/// A reply that read once and then announced its next read and stopped: that stream called no
/// tool, so it is asked (once). Words asking for no map or walkthrough: one that did is asked for
/// the drawing from what it read first (D-249, `sim_read_draw`).
#[test]
fn a_later_stream_that_announces_its_next_read_is_asked() {
    let mut sim = tour(repo_open());
    sim.talker
        .push(ScriptedReply::says("Listing it.").calls("list_dir", r#"{"path":"."}"#));
    sim.talker.push(ScriptedReply::says(
        "I see crates. Next I will look inside the crates folder.",
    ));
    sim.talker
        .push(ScriptedReply::says("Looking.").calls("list_dir", r#"{"path":"crates"}"#));
    sim.talker.push(ScriptedReply::says(DRAWS_CRATES));
    sim.user_says(0, "What is in this repo?");
    sim.run_until(60_000);
    assert_clean(&sim);
    assert_eq!(talks(&sim), 4);
    assert!(request_text(&sim, 2).contains(ASKED));
    assert_eq!(ran(&sim), 2);
}

/// A read comes before a drawing: a tour that was asked to draw and only announced its read
/// is asked for the read (D-180: what it draws of the repo comes from what it read); a read
/// written in the text is asked for as written.
#[test]
fn a_read_is_asked_for_before_a_drawing() {
    let mut sim = tour(repo_open());
    sim.talker.push(ScriptedReply::says(ANNOUNCES));
    sim.talker.push(ScriptedReply::says("Unused."));
    sim.user_says(0, "Draw me a map of this repo");
    sim.run_until(40_000);
    assert_clean(&sim);
    let asked = request_text(&sim, 1);
    assert!(asked.contains("Call list_dir now"), "{asked}");
    assert!(!asked.contains("they asked for a drawing"), "{asked}");
    let mut sim = tour(repo_open());
    sim.talker.push(ScriptedReply::says(
        r#"Let me look at the repo structure. list_dir path ".""#,
    ));
    sim.talker.push(ScriptedReply::says("Unused."));
    sim.user_says(0, "Walk me through this repo");
    sim.run_until(40_000);
    let asked = request_text(&sim, 1);
    assert!(asked.contains("wrote list_dir in its text"), "{asked}");
    assert!(!asked.contains(ASKED), "{asked}");
}

/// The user's turn wins. The announcement's stream ends while the user is saying "mm-hm" over
/// it (the floor is theirs until the backchannel is known), so nothing is asked; and starting to
/// draw during it stops the reply, with no request while they draw (I14).
#[test]
fn no_retry_once_the_user_has_the_floor_or_draws() {
    let mut sim = tour(repo_open());
    // "Sure." plays from the first chunk; the stream ends with its fifth, at 300 + 5 × 1,000 ms.
    sim.talker.first_token_ms = 300;
    sim.talker.chunk_interval_ms = 1_000;
    sim.talker
        .push(ScriptedReply::says(&format!("Sure. {ANNOUNCES}")));
    sim.talker.push(ScriptedReply::says("Unused."));
    sim.user_says(0, "Walk me through this repo");
    assert!(sim.run_until_true(10_000, |sim| !sim.talker.requests.is_empty()));
    let ends = sim.now().as_ms() + 5_300;
    sim.user_says_with_onset(ends - 300, "mm-hm");
    sim.run_until(ends);
    assert!(
        matches!(sim.core().state(), ConversationState::Ducked(_)),
        "the stream ends while the user holds the floor: {:?}",
        sim.core().state()
    );
    sim.run_until(ends + 30_000);
    assert_clean(&sim);
    assert_eq!(talks(&sim), 1, "the user had the floor");

    let mut sim = tour(repo_open());
    sim.talker.chunk_interval_ms = 400;
    sim.talker.push(ScriptedReply::says(ANNOUNCES));
    sim.user_says(0, "Walk me through this repo");
    assert!(sim.run_until_true(10_000, |sim| !sim.talker.requests.is_empty()));
    let at = sim.now().as_ms() + 600;
    sim.at(
        at,
        Input::Command(UiCommand::Draw {
            action: DrawAction::Start,
        }),
    );
    sim.run_until(at + 30_000);
    assert_clean(&sim);
    assert_eq!(talks(&sim), 1, "nothing is asked while the user draws");
}
