//! Simulator scenarios for tool calls the talker wrote as tags (D-143): in the owner's third
//! session gemma4 wrote `<remember …/>` and `<show_memory/>` into its speech, the parser dropped
//! them, and no note was kept while BoardAI said it was.
#![allow(clippy::expect_used, reason = "a broken scenario should fail loudly")]

mod common;

use boardai_core::conversation::ConversationState;
use boardai_core::effect::Effect;
use boardai_core::history::Message;
use boardai_core::input::Input;
use boardai_core::limits::TOOL_CALLS_PER_RESPONSE_MAX;
use boardai_core::memory::{MemoryKind, Subject};
use boardai_core::protocol::{ChipKind, DrawAction, UiCommand, UiEvent};
use boardai_core::sim::talker::ScriptedReply;
use boardai_core::sim::{Entry, Sim};
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

/// Every system message the talker was sent, in order.
fn all_notices(sim: &Sim) -> String {
    sim.talker
        .requests
        .iter()
        .flat_map(|request| &request.messages)
        .filter_map(|message| match message {
            Message::System { text } => Some(text.as_str()),
            Message::User { .. } | Message::Assistant { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn shown(sim: &Sim) -> bool {
    sim.ui()
        .any(|(_, event)| matches!(event, UiEvent::ViewShow { .. }))
}

/// The pen pressed while a view written as a tag is still being read: the read belongs to the
/// reply drawing stopped, so its answer shows nothing over the drawing.
#[test]
fn a_written_view_answered_after_drawing_starts_never_covers_the_drawing() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(
        "Here are your sessions. <show_library/> Pick one.",
    ));
    sim.user_says(0, "show me my sessions");
    assert!(sim.run_until_true(30_000, |sim| {
        sim.effects().any(|(_, effect)| {
            matches!(effect, Effect::RunTool { id, .. } if id.as_str().starts_with("written-"))
        })
    }));
    let at = sim.now().as_ms() + 1;
    sim.at(
        at,
        Input::Command(UiCommand::Draw {
            action: DrawAction::Start,
        }),
    );
    sim.run_until(at + 3_000);
    assert_clean(&sim);
    assert!(sim.core().is_drawing());
    assert!(!shown(&sim), "a view opened over the drawing");
    assert_eq!(sim.core().view(), None);
}

/// The same when the reply has already played to its end and only its read is left: the pen
/// still drops it.
#[test]
fn a_written_view_read_after_its_reply_ended_never_covers_a_drawing() {
    let mut sim = Sim::new(config());
    // The tag comes in a chunk of its own, long after the words before it have played.
    sim.talker.chunk_bytes = 24;
    sim.talker.chunk_interval_ms = 4_000;
    sim.talker.push(ScriptedReply::says(
        "Here are your sessions. <show_library/>",
    ));
    sim.user_says(0, "show me my sessions");
    assert!(sim.run_until_true(30_000, |sim| {
        sim.effects().any(|(_, effect)| {
            matches!(effect, Effect::RunTool { id, .. } if id.as_str().starts_with("written-"))
        })
    }));
    assert_eq!(
        *sim.core().state(),
        ConversationState::Idle,
        "the reply has ended"
    );
    let at = sim.now().as_ms() + 1;
    sim.at(
        at,
        Input::Command(UiCommand::Draw {
            action: DrawAction::Start,
        }),
    );
    sim.run_until(at + 3_000);
    assert_clean(&sim);
    assert!(sim.core().is_drawing());
    assert!(!shown(&sim), "a view opened over the drawing");
}

/// The user barges in while the talker's stream ends; the remark ("Redis?") then interrupts.
/// A view the stream called as a tool is dropped with the stop, and so is one written as a tag.
fn barge_in_as_the_stream_ends(sim: &mut Sim, reply: ScriptedReply, on_call: bool) {
    sim.talker.push(reply);
    sim.talker.push(ScriptedReply::says("Redis is the cache."));
    sim.user_says(0, "Walk me through the request path.");
    assert!(sim.run_until_true(20_000, |sim| {
        *sim.core().state() == ConversationState::Speaking
    }));
    let at = sim.now().as_ms() + 100;
    sim.at(at, Input::VoiceStart);
    assert!(sim.run_until_true(at + 20_000, |sim| {
        sim.trace().iter().any(|(_, entry)| {
            if on_call {
                matches!(entry, Entry::Input(Input::TalkerToolCall { .. }))
            } else {
                matches!(entry, Entry::Input(Input::TalkerDone { .. }))
            }
        })
    }));
    assert!(
        matches!(sim.core().state(), ConversationState::Ducked(_)),
        "{}",
        sim.core().state().name()
    );
    let done = sim.now().as_ms();
    sim.at(
        done + 2,
        Input::VoiceEnd {
            voiced_ms: 500,
            silence_ms: 0,
        },
    );
    sim.at(
        done + 4,
        Input::Transcript {
            text: "Redis?".to_owned(),
            is_final: true,
        },
    );
    sim.run_until(done + 20_000);
}

const LONG: &str = "Here are your past sessions, the request path first, then the cache, then \
                    the queue, and the last one about Raft.";

#[test]
fn a_view_written_as_a_tag_never_opens_after_the_user_interrupted() {
    let mut sim = Sim::new(config());
    let reply = ScriptedReply::says(&format!("{LONG} <show_library/>"));
    barge_in_as_the_stream_ends(&mut sim, reply, false);
    assert_clean(&sim);
    assert!(
        !shown(&sim),
        "the written view opened over the user's interruption"
    );
}

#[test]
fn a_view_called_as_a_tool_never_opens_after_the_user_interrupted() {
    let mut sim = Sim::new(config());
    let reply = ScriptedReply::says(LONG).calls("show_library", "{}");
    barge_in_as_the_stream_ends(&mut sim, reply, true);
    assert_clean(&sim);
    assert!(!shown(&sim));
}

/// `<open_session id="raft"/>` written as a tag, and no session matches: the talker hears that
/// nothing was shown, and why, once the read has answered.
#[test]
fn a_written_view_that_fails_is_reported_as_not_shown() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(
        "Here is the Raft session. <open_session id=\"raft\"/>",
    ));
    sim.talker.push(ScriptedReply::says("Okay."));
    sim.user_says(0, "Open my Raft session.");
    sim.run_until(10_000);
    sim.user_says(10_000, "What do you see?");
    sim.run_until(20_000);
    assert_clean(&sim);
    assert!(!shown(&sim));
    let told = all_notices(&sim);
    assert!(
        !told.contains("on screen"),
        "told a view it never got is on screen: {told}"
    );
    assert!(
        told.contains("<open_session id=\"raft\"/>") && told.contains("Nothing found."),
        "{told}"
    );
}

/// Past `TOOL_CALLS_PER_RESPONSE_MAX` tags in one stream, the rest are not read: the talker is
/// told how many (LLD §5).
#[test]
fn tool_tags_past_the_bound_are_counted_and_told() {
    let mut sim = Sim::new(config());
    let over = 3;
    let tags = "<add_task title=\"check the TTLs\"/>".repeat(TOOL_CALLS_PER_RESPONSE_MAX + over);
    sim.talker
        .push(ScriptedReply::says(&format!("Adding them. {tags}")));
    sim.talker.push(ScriptedReply::says("Okay."));
    sim.user_says(0, "Add a task to check the TTLs, a lot of times.");
    sim.run_until(10_000);
    sim.user_says(10_000, "Thanks.");
    sim.run_until(20_000);
    assert_clean(&sim);
    let stored = count(&sim, |effect| matches!(effect, Effect::Remember(_)));
    assert_eq!(stored, TOOL_CALLS_PER_RESPONSE_MAX);
    let told = all_notices(&sim);
    assert!(
        told.contains(&format!("{over} more tool tags")),
        "the dropped tags were not told: {told}"
    );
}
