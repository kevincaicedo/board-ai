//! Research calls held for a brief (D-223, D-268) and when the talker has a brief (D-271). A
//! `research` call about a brief the cache or the citation check is about to answer waits with
//! every other call about it, and all are answered when the harness starts, when the brief
//! arrives, and when its source stops being shared (removed from the tray, or its project left),
//! so a tool round never waits for nothing. A prefetched brief is the talker's only once a request
//! carried it. Review 3, findings 6, 13, 17 and 18: each test was red at 2b76809.
#![allow(clippy::expect_used, reason = "a broken scenario should fail loudly")]
#![allow(
    clippy::wildcard_enum_match_arm,
    reason = "scenarios pick one event kind"
)]

mod common;

use boardai_core::conversation::ConversationState;
use boardai_core::effect::Effect;
use boardai_core::input::Input;
use boardai_core::protocol::UiCommand;
use boardai_core::sim::Sim;
use boardai_core::sim::sources::code_folder;
use boardai_core::sim::talker::ScriptedReply;
use boardai_core::sources::tray::SourceAction;
use boardai_core::sources::{BriefTopic, SourceEffect, SourceId, SourceInput};
use common::{assert_clean, config, request_text};

const OVERVIEW: &str = "# Brief: inf-alloc overview
## Overview
inf-alloc hands out memory from arenas.
## Components
- arena · Arena · type · src/arena.rs — Owns pages and hands out slots
## Citations
- src/arena.rs:40-44
";

const ARENA: &str = "# Brief: the arena
## Overview
The arena keeps a free list per page and bumps a pointer inside the page.
## Components
- page · Page · type · src/arena.rs — 64 KiB of slots
## Citations
- src/arena.rs:60-62
";

const INF_ALLOC: &str = "~/…/infinitydb/crates/inf-alloc";

fn session() -> Sim {
    let mut sim = Sim::new(config());
    sim.sources.default_brief = OVERVIEW.to_owned();
    sim.sources.readable.insert("src/arena.rs".into(), 200);
    sim.sources.cache.insert(
        (INF_ALLOC.to_owned(), BriefTopic::component("Arena").key()),
        ARENA.to_owned(),
    );
    sim.at(
        0,
        Input::Source {
            source: SourceInput::Opened {
                harness: Some("Claude Code".into()),
                project: None,
            },
        },
    );
    sim.share(100, &code_folder(1, "inf-alloc", INF_ALLOC), false);
    sim.run_until(6_000);
    sim
}

fn arena_brief_asked(sim: &Sim) -> bool {
    sim.effects().any(|(_, effect)| {
        matches!(effect, Effect::Source { source: SourceEffect::Brief { topic, .. } }
            if *topic == BriefTopic::component("Arena"))
    })
}

/// Finding 13: the tray's Remove dropped the brief task, and the call held for its cached brief
/// was never answered: BoardAI stayed Thinking.
#[test]
fn removing_the_source_answers_a_call_held_for_its_cached_brief() {
    let mut sim = session();
    sim.talker.push(ScriptedReply::says("Let me check.").calls(
        "research",
        r#"{"question":"How does the Arena work?","depth":"quick"}"#,
    ));
    sim.talker.push(ScriptedReply::says("It bumps a pointer."));
    sim.user_says(7_000, "How does the Arena work in inf-alloc?");
    assert!(
        sim.run_until_true(20_000, arena_brief_asked),
        "the Arena brief is asked"
    );
    // The cache answers 5 ms later; the user removes the source from the tray meanwhile.
    let now = sim.now().as_ms();
    sim.at(
        now,
        Input::Command(UiCommand::Sources {
            action: SourceAction::Remove {
                source: SourceId(1),
            },
        }),
    );
    sim.run_until(60_000);
    assert_eq!(
        sim.talker.requests.len(),
        2,
        "the held research call is answered and the talker asked again"
    );
    assert!(
        request_text(&sim, 1).contains("is no longer shared"),
        "the call hears why"
    );
    assert_eq!(
        *sim.core().state(),
        ConversationState::Idle,
        "BoardAI is not stuck"
    );
    assert_clean(&sim);
}

/// Finding 18: switching projects dropped the old project's kept sources but, unlike the tray's
/// Remove, left their briefs running: the harness was never cancelled, its citations were still
/// read for a discarded source, and the brief the talker was promised was silently lost.
#[test]
fn switching_projects_stops_a_discarded_sources_brief() {
    let mut sim = Sim::new(config());
    sim.sources.default_brief = OVERVIEW.to_owned();
    sim.sources
        .briefs
        .insert(BriefTopic::component("Arena").key(), ARENA.to_owned());
    sim.sources.readable.insert("src/arena.rs".into(), 200);
    sim.at(
        0,
        Input::Source {
            source: SourceInput::Opened {
                harness: Some("Claude Code".into()),
                project: None,
            },
        },
    );
    // Shared on the right half: kept in a new project named after it.
    sim.share(100, &code_folder(1, "inf-alloc", INF_ALLOC), true);
    sim.run_until(6_000);
    sim.talker
        .push(ScriptedReply::says("Claude Code is reading the arena."));
    sim.talker.push(ScriptedReply::says("Working on it."));
    sim.user_says(7_000, "More detail on the arena, please.");
    assert!(sim.run_until_true(20_000, arena_brief_asked));
    let task = sim
        .effects()
        .find_map(|(_, effect)| match effect {
            Effect::Source {
                source: SourceEffect::Brief { task, topic, .. },
            } if *topic == BriefTopic::component("Arena") => Some(*task),
            _ => None,
        })
        .expect("the arena brief's task");
    let asked_at = sim.now().as_ms();
    sim.user_says(asked_at + 200, "We're working on infinitydb.");
    sim.run_until(60_000);
    let discarded = sim.effects().any(|(_, effect)| {
        matches!(effect, Effect::Source { source: SourceEffect::Discard { sources } }
            if sources.contains(&SourceId(1)))
    });
    assert!(discarded, "the old project's source is discarded");
    let checked_after_discard = sim
        .effects()
        .skip_while(|(_, effect)| {
            !matches!(effect, Effect::Source { source: SourceEffect::Discard { sources } }
                if sources.contains(&SourceId(1)))
        })
        .any(|(_, effect)| {
            matches!(effect, Effect::Source { source: SourceEffect::CheckCitations { source, .. } }
                if *source == SourceId(1))
        });
    let cancelled = sim.effects().any(|(_, effect)| {
        matches!(effect, Effect::CancelHarness { task: cancelled } if *cancelled == task)
    });
    assert!(
        cancelled && !checked_after_discard,
        "the discarded source's brief is cancelled (cancelled: {cancelled}, citations read \
         after the discard: {checked_after_discard})"
    );
}

/// Finding 6: two `research` calls in one response about the same part (models call tools in
/// parallel). The second found the first's brief on its way and took its place, so neither the
/// harness starting nor the brief arriving answered the first: the tool round never completed.
#[test]
fn two_research_calls_about_one_part_are_both_answered() {
    let mut sim = Sim::new(config());
    sim.sources.default_brief = OVERVIEW.to_owned();
    sim.sources
        .briefs
        .insert(BriefTopic::component("Arena").key(), ARENA.to_owned());
    sim.sources.readable.insert("src/arena.rs".into(), 200);
    sim.at(
        0,
        Input::Source {
            source: SourceInput::Opened {
                harness: Some("Claude Code".into()),
                project: None,
            },
        },
    );
    sim.share(100, &code_folder(1, "inf-alloc", INF_ALLOC), false);
    sim.run_until(6_000);
    sim.talker.push(
        ScriptedReply::says("Let me check both.")
            .calls(
                "research",
                r#"{"question":"How does the arena allocate a slot?","depth":"quick"}"#,
            )
            .calls(
                "research",
                r#"{"question":"How does the arena free a slot?","depth":"quick"}"#,
            ),
    );
    sim.talker
        .push(ScriptedReply::says("Claude Code is still reading."));
    // A provider's parallel calls arrive back to back, before the runtime's cache lookup answers.
    sim.talker.chunk_interval_ms = 1;
    sim.user_says(7_000, "How does the arena allocate and free slots?");
    sim.run_until(60_000);
    assert_eq!(
        sim.talker.requests.len(),
        3,
        "both calls answered (the tool round), then the brief's report turn"
    );
    assert_clean(&sim);
}

/// The same with the brief in the cache: the first call gets it, the second is pointed at it,
/// in one tool round.
#[test]
fn two_research_calls_about_a_cached_part_share_its_brief() {
    let mut sim = session();
    sim.talker.push(
        ScriptedReply::says("Let me check both.")
            .calls(
                "research",
                r#"{"question":"How does the Arena allocate a slot?","depth":"quick"}"#,
            )
            .calls(
                "research",
                r#"{"question":"How does the Arena free a slot?","depth":"quick"}"#,
            ),
    );
    sim.talker.push(ScriptedReply::says("It bumps a pointer."));
    sim.talker.chunk_interval_ms = 1;
    sim.user_says(7_000, "How does the Arena allocate and free slots?");
    sim.run_until(40_000);
    assert_eq!(sim.talker.requests.len(), 2, "one tool round");
    let round = request_text(&sim, 1);
    assert_eq!(round.matches("# Brief: the arena").count(), 1, "{round}");
    assert!(round.contains("your other research call"), "{round}");
    assert_eq!(*sim.core().state(), ConversationState::Idle);
    assert_clean(&sim);
}

/// Finding 17: a prefetched brief that arrived while a reply was in flight was marked delivered at
/// once, but its text waited for the next new turn; a `research` call in a later round of the
/// same reply was told it was "already in this conversation" when no request had carried it.
#[test]
fn a_prefetch_is_the_talkers_only_once_a_request_carried_it() {
    let mut sim = Sim::new(config());
    sim.sources.default_brief = OVERVIEW.to_owned();
    sim.sources.readable.insert("src/arena.rs".into(), 200);
    sim.at(
        0,
        Input::Source {
            source: SourceInput::Opened {
                harness: Some("Claude Code".into()),
                project: None,
            },
        },
    );
    sim.share(
        100,
        &code_folder(1, "inf-alloc", "~/…/infinitydb/crates/inf-alloc"),
        false,
    );
    // A long first stream: the overview (3 s of harness) arrives while it is still speaking.
    let long = "Allocators are a big topic, so let me start with the general idea. ".repeat(24);
    sim.talker
        .push(ScriptedReply::says(&long).calls("list_dir", r#"{"path":"."}"#));
    sim.talker.push(ScriptedReply::says("Now the brief.").calls(
        "research",
        r#"{"question":"How does inf-alloc work?","depth":"quick"}"#,
    ));
    sim.talker.push(ScriptedReply::says("It hands out slots."));
    sim.user_says(200, "How does inf-alloc work?");
    sim.run_until(90_000);
    let requests = sim.talker.requests.len();
    assert!(requests >= 3, "three rounds: {requests}");
    // Whenever the talker hears the brief is in the conversation, a request carried it.
    for index in 0..requests {
        if request_text(&sim, index).contains("already in this conversation") {
            let sent_by_then = (0..=index)
                .any(|before| request_text(&sim, before).contains("# Brief: inf-alloc overview"));
            assert!(
                sent_by_then,
                "request {index} claims a brief no request carried"
            );
        }
    }
    let research_answer = request_text(&sim, 2);
    assert!(
        research_answer.contains("# Brief: inf-alloc overview"),
        "the research call carries the waiting brief: {research_answer}"
    );
    assert_eq!(
        (0..requests)
            .map(|index| request_text(&sim, index)
                .matches("# Brief: inf-alloc overview")
                .count())
            .max(),
        Some(1),
        "the brief goes once"
    );
    assert_clean(&sim);
}
