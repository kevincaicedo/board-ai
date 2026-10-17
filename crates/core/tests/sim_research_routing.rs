//! Where a `research` call goes with documents shared (D-256, D-267): to a document's digest only
//! when its words point at that document (its file's name or its digest's title written out,
//! "the paper"), and never when they name the code; anything else is the code's, through a brief
//! when a code source is shared, else the harness. Review 3, finding 4: the first two tests were
//! red at 2b76809, one in each direction.
#![allow(clippy::expect_used, reason = "a broken scenario should fail loudly")]
#![allow(
    clippy::wildcard_enum_match_arm,
    reason = "scenarios pick one event kind out of the whole trace"
)]

mod common;

use boardai_core::effect::Effect;
use boardai_core::history::{Message, UserPart};
use boardai_core::input::Input;
use boardai_core::sim::Sim;
use boardai_core::sim::sources::code_folder;
use boardai_core::sim::talker::ScriptedReply;
use boardai_core::sources::{SourceEffect, SourceInfo, SourceInput};
use common::documents::{DIGEST, opened, paper, read_event};
use common::{assert_clean, config};

/// The result the talker got for its first tool call, in request 1.
fn first_result(sim: &Sim) -> String {
    sim.talker.requests[1]
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::User { parts } => Some(parts),
            _ => None,
        })
        .flatten()
        .filter_map(|part| match part {
            UserPart::ToolResult { content, .. } => Some(content.clone()),
            UserPart::Text { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn briefs(sim: &Sim) -> usize {
    sim.effects()
        .filter(|(_, effect)| {
            matches!(
                effect,
                Effect::Source {
                    source: SourceEffect::Brief { .. }
                }
            )
        })
        .count()
}

fn harness_runs(sim: &Sim) -> usize {
    sim.effects()
        .filter(|(_, effect)| matches!(effect, Effect::RunHarness { .. }))
        .count()
}

/// A code folder and the paper, both shared; the overview brief has arrived.
fn code_and_paper() -> Sim {
    let mut sim = Sim::new(config());
    sim.at(
        0,
        Input::Source {
            source: SourceInput::Opened {
                harness: Some("Claude Code".into()),
                project: None,
            },
        },
    );
    sim.sources.default_brief = "# Brief: inf-alloc overview\n## Overview\nArenas.\n".to_owned();
    sim.share(100, &code_folder(1, "inf-alloc", "~/code/inf-alloc"), false);
    sim.share(200, &paper(), false);
    sim.at(600, read_event(DIGEST));
    sim.run_until(8_000);
    sim
}

fn asks(sim: &mut Sim, question: &str) {
    let input = format!(r#"{{"question":"{question}","depth":"quick"}}"#);
    sim.talker
        .push(ScriptedReply::says("Let me check.").calls("research", &input));
    sim.talker
        .push(ScriptedReply::says("Here is what I found."));
    let at = sim.now().as_ms() + 1_000;
    sim.user_says(at, question);
    sim.run_until(at + 30_000);
}

/// Finding 4: with a code folder shared, every research call went to the code first, so a
/// question naming the paper started a harness brief about the code folder.
#[test]
fn a_question_naming_the_paper_is_answered_from_its_digest_with_code_shared() {
    let mut sim = code_and_paper();
    let before = briefs(&sim);
    asks(
        &mut sim,
        "What does the layered store paper say about recovery?",
    );
    let answer = first_result(&sim);
    assert_eq!(briefs(&sim), before, "no brief about the code: {answer}");
    assert!(
        answer.contains("BoardAI read layered-store.pdf: its digest is in this conversation")
            && !answer.contains("inf-alloc"),
        "the paper's digest answers: {answer}"
    );
    assert_clean(&sim);
}

/// Finding 4, the other way: a word of five letters or more from a document's name sent a
/// question about the repository to the paper's digest.
#[test]
fn a_question_about_the_code_is_not_taken_by_a_documents_word() {
    let mut sim = Sim::new(config());
    sim.at(0, opened());
    let storage = SourceInfo {
        title: "storage-engines.pdf".into(),
        shown_path: "~/Papers/storage-engines.pdf".into(),
        ..paper()
    };
    sim.share(100, &storage, false);
    sim.at(600, read_event(DIGEST));
    sim.run_until(1_000);
    asks(
        &mut sim,
        "How does compaction work in this repository's storage layer?",
    );
    let answer = first_result(&sim);
    assert!(
        !answer.contains("digest"),
        "a question about the repository went to the paper: {answer}"
    );
    assert_eq!(harness_runs(&sim), 1, "the harness researches the code");
}

/// "The paper" points at the shared document; a question naming the code goes to the code
/// even when it mentions the paper too.
#[test]
fn the_paper_points_at_the_document_and_the_code_wins_a_tie() {
    let mut sim = code_and_paper();
    asks(&mut sim, "What does the paper say about recovery?");
    assert!(
        first_result(&sim).contains("BoardAI read layered-store.pdf"),
        "\"the paper\" is the shared one"
    );
    let mut sim = code_and_paper();
    let before = briefs(&sim);
    asks(
        &mut sim,
        "Does our code recover the way the layered store paper says?",
    );
    let answer = first_result(&sim);
    assert!(
        !answer.contains("Digest") && answer.contains("inf-alloc"),
        "the code's: {answer}"
    );
    assert!(briefs(&sim) >= before, "a brief, never the digest");
}

/// A question that points at nothing goes to the shared code, never to the paper.
#[test]
fn a_question_that_names_nothing_goes_to_the_code() {
    let mut sim = code_and_paper();
    asks(&mut sim, "How does recovery work?");
    let answer = first_result(&sim);
    assert!(
        !answer.contains("Digest") && answer.contains("inf-alloc"),
        "the code source answers: {answer}"
    );
}
