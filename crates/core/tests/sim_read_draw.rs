//! Simulator scenarios for the drawing asked for after reads (D-249): a map, a walkthrough of a
//! whole thing or a drawing was asked for, a repo read of the reply returned, and the reply drew
//! nothing. gemma4 read once, then announced its next read and stopped with D-178's one retry
//! spent: 2 of 10 tours drew (T-033). The core asks once a turn for the drawing, from what was
//! read, with its own allowance (`READ_DRAW_NUDGES_PER_TURN_MAX`) on top of D-178's.
#![allow(clippy::expect_used, reason = "a broken scenario should fail loudly")]

mod common;

use boardai_core::effect::Effect;
use boardai_core::sim::Sim;
use boardai_core::sim::talker::ScriptedReply;
use boardai_core::tools::{DirEntry, ToolFailure, ToolOutput};
use boardai_core::{CoreConfig, RepoReads};
use common::{assert_clean, config, count, op_starts};

/// gemma4's first turn, as measured.
const ANNOUNCES: &str = "I will start by listing the root directory to see the structure.";

/// gemma4 after its first listing (T-033): it names the next read and stops.
const ANNOUNCES_NEXT: &str = "I see crates. Next I will look inside the crates folder.";

const DRAWS_CRATES: &str =
    r#"The code lives in <node id="crates" label="crates/" kind="group"/>crates."#;

/// What the drawing-after-reads note says.
const ASKED_TO_DRAW: &str = "you read the repo this turn but drew nothing";

/// What D-232's note says.
const ASKED_TO_READ: &str = "said it would look at the repo but called no tool";

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

/// The requests whose last note is `note`'s.
fn asked(sim: &Sim, note: &str) -> usize {
    (0..sim.talker.requests.len())
        .filter(|index| {
            let messages = &sim.talker.requests[*index].messages;
            messages
                .iter()
                .rev()
                .find(|message| !matches!(message, boardai_core::history::Message::System { .. }))
                .is_some_and(|last| format!("{last:?}").contains(note))
        })
        .count()
}

fn tour(config: CoreConfig) -> Sim {
    let mut sim = Sim::new(config);
    for _ in 0..4 {
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
    }
    sim
}

/// T-033's tour: the first stream announces and is asked for the read (D-232); it reads; the
/// next announces its next read and stops. That retry is spent, and this one asks for the drawing
/// from what was read, which comes.
#[test]
fn a_tour_that_read_and_drew_nothing_is_asked_for_the_drawing() {
    let mut sim = tour(repo_open());
    sim.talker.push(ScriptedReply::says(ANNOUNCES));
    sim.talker
        .push(ScriptedReply::says("Listing it now.").calls("list_dir", r#"{"path":"."}"#));
    sim.talker.push(ScriptedReply::says(ANNOUNCES_NEXT));
    sim.talker.push(ScriptedReply::says(DRAWS_CRATES));
    sim.user_says(0, "Walk me through this repo");
    sim.run_until(60_000);
    assert_clean(&sim);
    assert_eq!(
        talks(&sim),
        4,
        "the read asked for, the tool round, the drawing asked for"
    );
    assert_eq!(asked(&sim, ASKED_TO_READ), 1);
    assert_eq!(asked(&sim, ASKED_TO_DRAW), 1);
    let note = request_text(&sim, 3);
    assert!(note.contains(ASKED_TO_DRAW), "{note}");
    assert!(
        note.contains(ANNOUNCES_NEXT),
        "what it said stays in the history: {note}"
    );
    assert_eq!(ran(&sim), 1);
    let drawn = op_starts(&sim);
    assert_eq!(drawn.len(), 1, "it drew what it read");
    assert!(
        format!("{:?}", drawn[0].1).contains("label: \"crates/\""),
        "a name the listing returned, unmarked: {drawn:?}"
    );
}

/// Once something was read, the drawing is what is missing: a later stream that announces its
/// next read is asked for the drawing, not for the read; D-232's retry stays for later.
#[test]
fn after_a_read_the_drawing_comes_before_the_next_read() {
    let mut sim = tour(repo_open());
    sim.talker
        .push(ScriptedReply::says("Listing it.").calls("list_dir", r#"{"path":"."}"#));
    sim.talker.push(ScriptedReply::says(ANNOUNCES_NEXT));
    sim.talker.push(ScriptedReply::says(DRAWS_CRATES));
    sim.user_says(0, "Walk me through this repo");
    sim.run_until(60_000);
    assert_clean(&sim);
    assert_eq!(talks(&sim), 3);
    assert!(request_text(&sim, 2).contains(ASKED_TO_DRAW));
    assert_eq!(asked(&sim, ASKED_TO_READ), 0);
    assert_eq!(op_starts(&sim).len(), 1);
}

/// The bound: the drawing is asked for once a turn, then D-178's one retry; a reply that still
/// draws nothing ends as it is. The next turn has both again.
#[test]
fn the_drawing_after_reads_is_asked_for_once_a_turn() {
    let mut sim = tour(repo_open());
    sim.talker
        .push(ScriptedReply::says("Listing it.").calls("list_dir", r#"{"path":"."}"#));
    sim.talker
        .push(ScriptedReply::says("I see a crates folder and some docs."));
    sim.talker.push(ScriptedReply::says(ANNOUNCES_NEXT));
    sim.talker.push(ScriptedReply::says(
        "Sure, the crates folder holds the code.",
    ));
    sim.user_says(0, "Walk me through this repo");
    sim.run_until(60_000);
    assert_eq!(
        talks(&sim),
        4,
        "the tool round, the drawing, then the read asked for"
    );
    assert_eq!(asked(&sim, ASKED_TO_DRAW), 1);
    assert_eq!(asked(&sim, ASKED_TO_READ), 1);
    assert!(op_starts(&sim).is_empty());
    sim.talker
        .push(ScriptedReply::says("Listing it.").calls("list_dir", r#"{"path":"."}"#));
    sim.talker
        .push(ScriptedReply::says("I see a crates folder and some docs."));
    sim.talker.push(ScriptedReply::says(DRAWS_CRATES));
    sim.user_says(60_000, "Draw me a map of this repo");
    sim.run_until(120_000);
    assert_clean(&sim);
    assert_eq!(talks(&sim), 7, "a new turn may ask again");
    assert_eq!(asked(&sim, ASKED_TO_DRAW), 2);
}

/// Only a map, a walkthrough of a whole thing or a drawing asks for one: a question answered from
/// a file read is answered in words.
#[test]
fn no_drawing_is_asked_for_when_none_was_wanted() {
    let mut sim = tour(repo_open());
    sim.talker
        .push(ScriptedReply::says("Let me read it.").calls("read_file", r#"{"path":"README.md"}"#));
    sim.talker.push(ScriptedReply::says(
        "The readme says the app runs on macOS, as README.md:3 shows.",
    ));
    sim.user_says(0, "What does the readme say about platforms?");
    sim.run_until(40_000);
    assert_clean(&sim);
    assert_eq!(talks(&sim), 2, "the tool round only");
    assert_eq!(asked(&sim, ASKED_TO_DRAW), 0);
}

/// A reply that drew after reading is never asked; nor is one whose reads returned nothing (it
/// has nothing to draw from), which gets D-232's retry as before.
#[test]
fn no_drawing_is_asked_for_after_a_drawing_or_without_a_read() {
    let mut sim = tour(repo_open());
    sim.talker
        .push(ScriptedReply::says("Listing it.").calls("list_dir", r#"{"path":"."}"#));
    sim.talker.push(ScriptedReply::says(DRAWS_CRATES));
    sim.user_says(0, "Walk me through this repo");
    sim.run_until(40_000);
    assert_eq!(talks(&sim), 2, "it drew");

    let mut sim = Sim::new(repo_open());
    sim.tools.respond(
        "list_dir",
        ToolOutput::Failed {
            kind: ToolFailure::NotFound,
            message: "no such folder".to_owned(),
        },
    );
    sim.talker
        .push(ScriptedReply::says("Listing it.").calls("list_dir", r#"{"path":"src"}"#));
    sim.talker.push(ScriptedReply::says(ANNOUNCES_NEXT));
    sim.talker.push(ScriptedReply::says("Unused."));
    sim.user_says(0, "Walk me through this repo");
    sim.run_until(40_000);
    assert_clean(&sim);
    assert_eq!(asked(&sim, ASKED_TO_DRAW), 0, "nothing was read");
    assert_eq!(asked(&sim, ASKED_TO_READ), 1);
}

/// The general rules hold for this note too: never while the user draws (I14).
#[test]
fn no_drawing_is_asked_for_once_the_user_draws() {
    use boardai_core::input::Input;
    use boardai_core::protocol::{DrawAction, UiCommand};
    let mut sim = tour(repo_open());
    sim.talker.chunk_interval_ms = 400;
    sim.talker
        .push(ScriptedReply::says("Listing it.").calls("list_dir", r#"{"path":"."}"#));
    sim.talker.push(ScriptedReply::says(ANNOUNCES_NEXT));
    sim.talker.push(ScriptedReply::says("Unused."));
    sim.user_says(0, "Walk me through this repo");
    assert!(sim.run_until_true(20_000, |sim| sim.talker.requests.len() == 2));
    let at = sim.now().as_ms() + 600;
    sim.at(
        at,
        Input::Command(UiCommand::Draw {
            action: DrawAction::Start,
        }),
    );
    sim.run_until(at + 30_000);
    assert_clean(&sim);
    assert_eq!(talks(&sim), 2, "nothing is asked while the user draws");
    assert_eq!(asked(&sim, ASKED_TO_DRAW), 0);
}
