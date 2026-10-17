//! Simulator scenarios for tool calls the talker wrote as tags (D-143): in the owner's third
//! session gemma4 wrote `<remember …/>` and `<show_memory/>` into its speech, the parser dropped
//! them, and no note was kept while BoardAI said it was.
#![allow(clippy::expect_used, reason = "a broken scenario should fail loudly")]

mod common;

use boardai_core::effect::Effect;
use boardai_core::history::Message;
use boardai_core::memory::{MemoryKind, Subject};
use boardai_core::protocol::{ChipKind, UiEvent};
use boardai_core::sim::Sim;
use boardai_core::sim::talker::ScriptedReply;
use boardai_core::views::ViewKind;
use common::{assert_clean, config, count};

fn notices(sim: &Sim) -> String {
    sim.talker
        .requests
        .last()
        .map(|request| {
            request
                .messages
                .iter()
                .filter_map(|message| match message {
                    Message::System { text } => Some(text.as_str()),
                    Message::User { .. } | Message::Assistant { .. } => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// The owner's session 3, word for word: the note is kept, its chip flies, and the talker
/// hears it was carried out.
#[test]
fn a_note_written_as_a_tag_is_kept() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(
        "I can create that note for you. <remember kind=\"note\" text=\"story consensus \
         protocol\" about=\"you\"/> I have made a note for you about the story consensus \
         protocol.",
    ));
    sim.talker.push(ScriptedReply::says("Sure."));
    sim.user_says(0, "Let's create one note: review the consensus protocol.");
    sim.run_until(15_000);
    let written: Vec<_> = sim
        .effects()
        .filter_map(|(_, effect)| {
            if let Effect::Remember(write) = effect {
                Some(write.clone())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(written.len(), 1, "the note reaches the store");
    assert_eq!(written[0].kind, MemoryKind::Note);
    assert_eq!(written[0].text, "story consensus protocol");
    assert_eq!(written[0].subject, Subject::You);
    let chip = sim.ui().any(|(_, event)| {
        matches!(event, UiEvent::Chip { kind: ChipKind::Note, text, .. } if text == "story consensus protocol")
    });
    assert!(chip, "the note's chip flies");
    sim.user_says(15_000, "thanks, what's next?");
    sim.run_until(25_000);
    assert_clean(&sim);
    let told = notices(&sim);
    assert!(
        told.contains("wrote tool calls as tags") && told.contains("carried out as a call: Saved."),
        "{told}"
    );
}

/// "Show me my notes" answered with `<show_memory/>`: the view is read and shown.
#[test]
fn a_view_written_as_a_tag_is_shown() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(
        "Here is what I remember about you and your systems. <show_memory/>",
    ));
    sim.user_says(0, "Can you show me my notes?");
    sim.run_until(15_000);
    assert_clean(&sim);
    let reads = count(&sim, |effect| matches!(effect, Effect::RunTool { .. }));
    assert_eq!(reads, 1);
    assert_eq!(sim.core().view(), Some(ViewKind::Memory));
}

/// A tool whose result is for the talker, or that needs a yes, runs nothing from a tag.
#[test]
fn a_read_or_a_change_written_as_a_tag_runs_nothing() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(
        "Let me look. <read_file path=\"src/main.rs\"/> <propose_change instruction=\"rename \
         it\"/> Done.",
    ));
    sim.talker.push(ScriptedReply::says("Okay."));
    sim.user_says(0, "Read main and rename it.");
    sim.run_until(10_000);
    sim.user_says(10_000, "and?");
    sim.run_until(20_000);
    assert_clean(&sim);
    let ran = count(&sim, |effect| {
        matches!(effect, Effect::RunTool { .. } | Effect::RunHarness { .. })
    });
    assert_eq!(ran, 0);
    let told = notices(&sim);
    assert!(
        told.contains("nothing ran. Call read_file as a tool") && told.contains("propose_change"),
        "{told}"
    );
}

/// "Don't remember this session" holds for a tag as for a call (I10).
#[test]
fn a_tag_stores_nothing_off_the_record() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(
        "Noted. <add_task title=\"Check Redis TTLs\"/>",
    ));
    sim.talker.push(ScriptedReply::says("Okay."));
    sim.user_says(0, "don't remember this session");
    sim.run_until(5_000);
    sim.user_says(5_000, "add a task to check Redis TTLs");
    sim.run_until(15_000);
    sim.user_says(15_000, "thanks");
    sim.run_until(25_000);
    assert_clean(&sim);
    let stored = count(&sim, |effect| matches!(effect, Effect::Remember(_)));
    assert_eq!(stored, 0);
    assert!(notices(&sim).contains("Not stored"), "{}", notices(&sim));
}

/// An inferred memory written as a tag waits in the digest: nobody asked the user about it, so
/// their next "yes" (to something else) keeps nothing (the proposal gate's rule, I9).
#[test]
fn an_inferred_memory_written_as_a_tag_opens_no_gate() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(
        "Diagrams it is. <remember kind=\"inferred\" text=\"likes diagrams first\" \
         about=\"you\"/> Shall I draw the cache next?",
    ));
    sim.talker.push(ScriptedReply::says("Drawing it."));
    sim.user_says(0, "I like seeing it drawn.");
    sim.run_until(12_000);
    let proposed = count(&sim, |effect| matches!(effect, Effect::Remember(_)));
    assert_eq!(proposed, 1, "it is proposed, into the digest");
    sim.user_says(12_000, "yes");
    sim.run_until(24_000);
    assert_clean(&sim);
    let kept = count(&sim, |effect| {
        matches!(effect, Effect::AcceptProposal { .. })
    });
    assert_eq!(kept, 0, "a yes to the drawing question keeps nothing");
}
