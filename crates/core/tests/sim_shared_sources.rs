//! Sources (PRODUCT §7.7, D-184) and research briefs (D-185) in the simulator: sharing a code
//! folder starts its overview, a question uses the cached brief, "more detail on X" asks for a
//! component brief, an unread citation never shows a chip, a kept source is there next
//! session, names said by voice are resolved (or asked about), and briefs wait for a drawing
//! and die with the session.
#![allow(clippy::expect_used, reason = "a broken scenario should fail loudly")]
#![allow(
    clippy::wildcard_enum_match_arm,
    reason = "scenarios pick one event kind out of the whole trace"
)]

mod common;

use boardai_core::effect::Effect;
use boardai_core::history::{Message, UserPart};
use boardai_core::input::Input;
use boardai_core::protocol::{ChipKind, DrawAction, UiCommand, UiEvent};
use boardai_core::sim::Sim;
use boardai_core::sim::sources::code_folder;
use boardai_core::sim::talker::ScriptedReply;
use boardai_core::sources::tray::SourcesTray;
use boardai_core::sources::{
    BriefTopic, Project, SourceEffect, SourceId, SourceInput, SourceState,
};
use common::{assert_clean, config};

const OVERVIEW: &str = "# Brief: inf-alloc overview
## Overview
inf-alloc hands out memory from arenas. Each arena carves slots from aligned pages.
## Components
- arena · Arena · type · src/arena.rs — Owns pages and hands out slots
- slot-pool · Slot pool · module · src/pool/slot.rs — Recycles freed slots
## Relations
- arena → slot-pool: returns freed slots
## Flows
### Allocate
1. The arena takes a free slot (src/arena.rs:42)
2. A step from a file that isn't there (src/gone.rs:9)
## Diagram
<diagram title=\"inf-alloc\">
<node id=\"arena\" label=\"Arena\" kind=\"service\"/>
<node id=\"slot-pool\" label=\"Slot pool\" kind=\"service\"/>
<edge from=\"arena\" to=\"slot-pool\" label=\"frees\"/>
</diagram>
## Citations
- src/arena.rs:40-44
- src/gone.rs:9
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

/// A session with Claude Code installed, and a scripted runtime that knows inf-alloc's files.
fn session() -> Sim {
    let mut sim = Sim::new(config());
    sim.sources.default_brief = OVERVIEW.to_owned();
    sim.sources
        .briefs
        .insert(BriefTopic::component("Arena").key(), ARENA.to_owned());
    sim.sources.touched = vec!["src/arena.rs".into(), "src/pool/slot.rs".into()];
    sim.sources.readable.insert("src/arena.rs".into(), 200);
    sim.sources.readable.insert("src/pool/slot.rs".into(), 80);
    opened(&mut sim, None);
    sim
}

fn opened(sim: &mut Sim, project: Option<Project>) {
    sim.at(
        0,
        Input::Source {
            source: SourceInput::Opened {
                harness: Some("Claude Code".into()),
                project,
            },
        },
    );
}

fn trays(sim: &Sim) -> Vec<SourcesTray> {
    sim.ui()
        .filter_map(|(_, event)| match event {
            UiEvent::Sources { tray } => Some(tray.clone()),
            _ => None,
        })
        .collect()
}

fn last_tray(sim: &Sim) -> SourcesTray {
    trays(sim).last().cloned().expect("a tray")
}

fn source_effects(sim: &Sim) -> Vec<SourceEffect> {
    sim.effects()
        .filter_map(|(_, effect)| match effect {
            Effect::Source { source } => Some(source.clone()),
            _ => None,
        })
        .collect()
}

fn said(sim: &Sim) -> Vec<String> {
    sim.effects()
        .filter_map(|(_, effect)| match effect {
            Effect::Synthesize { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn briefs_asked(sim: &Sim) -> Vec<(SourceId, BriefTopic)> {
    source_effects(sim)
        .into_iter()
        .filter_map(|effect| match effect {
            SourceEffect::Brief { source, topic, .. } => Some((source, topic)),
            _ => None,
        })
        .collect()
}

/// Every user-side text the talker was sent in request `index`.
fn user_texts(sim: &Sim, index: usize) -> Vec<String> {
    sim.talker.requests[index]
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::User { parts } => Some(parts),
            _ => None,
        })
        .flatten()
        .map(|part| match part {
            UserPart::Text { text } => text.clone(),
            UserPart::ToolResult { content, .. } => content.clone(),
        })
        .collect()
}

fn system_texts(sim: &Sim, index: usize) -> String {
    sim.talker.requests[index]
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::System { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn source_chips(sim: &Sim) -> Vec<String> {
    sim.ui()
        .filter_map(|(_, event)| match event {
            UiEvent::Chip {
                kind: ChipKind::Source,
                source: Some(source),
                ..
            } => Some(format!("{}:{}", source.path, source.line)),
            _ => None,
        })
        .collect()
}

/// Shares inf-alloc and lets its overview arrive.
fn share_and_read(sim: &mut Sim) {
    sim.share(100, &code_folder(1, "inf-alloc", INF_ALLOC), false);
    sim.run_until(6_000);
}

#[test]
fn sharing_a_code_folder_starts_its_overview_and_the_tray_follows_the_reading() {
    let mut sim = session();
    share_and_read(&mut sim);
    assert_eq!(
        briefs_asked(&sim),
        vec![(SourceId(1), BriefTopic::Overview)],
        "one overview, at once"
    );
    let reading: Vec<SourcesTray> = trays(&sim)
        .into_iter()
        .filter(|tray| {
            matches!(
                tray.session.first().map(|row| &row.state),
                Some(SourceState::Reading { .. })
            )
        })
        .collect();
    assert!(!reading.is_empty(), "the tray shows the reading");
    assert!(reading.iter().all(|tray| tray.open), "it opens on its own");
    assert!(
        reading
            .iter()
            .any(|tray| tray.reading.contains(&"src/pool/slot.rs".to_owned())),
        "the harness's files are the ghost names"
    );
    let tray = last_tray(&sim);
    assert!(!tray.open && tray.reading.is_empty(), "it closes once read");
    assert!(
        matches!(&tray.session[0].state, SourceState::Partial { reason } if reason.contains("2 parts mapped")),
        "{:?}",
        tray.session[0].state
    );
    let project = tray.project.expect("a project proposed from the folder");
    assert!(project.proposed && project.name == "inf-alloc");
    assert!(said(&sim).contains(&"Claude Code is reading inf-alloc.".to_owned()));
    assert!(
        source_effects(&sim).iter().any(|effect| matches!(
            effect,
            SourceEffect::CacheBrief {
                topic: BriefTopic::Overview,
                ..
            }
        )),
        "the brief is cached for the next question"
    );
    assert_clean(&sim);
}

#[test]
fn a_question_about_the_source_uses_the_brief_and_unread_citations_never_chip() {
    let mut sim = session();
    share_and_read(&mut sim);
    sim.talker
        .push(ScriptedReply::says("Let me check the brief.").calls(
            "research",
            r#"{"question":"How does inf-alloc hand out memory?","depth":"quick"}"#,
        ));
    sim.talker.push(ScriptedReply::says(
        "The arena takes a free slot, as src/arena.rs:42 shows, and src/gone.rs:9 is cited too.",
    ));
    sim.user_says(7_000, "How does inf-alloc hand out memory?");
    sim.run_until(20_000);
    // The prefetched brief went to the talker with the question, fenced and without the
    // citation nobody could read.
    let first = user_texts(&sim, 0).join("\n");
    assert!(
        first.contains("<task-output>\n# Brief: inf-alloc overview"),
        "{first}"
    );
    assert!(!first.contains("src/gone.rs"), "{first}");
    assert!(
        first.contains("couldn't be read and were removed"),
        "{first}"
    );
    assert!(system_texts(&sim, 0).contains("For any question about it, call research"));
    // The research call is answered from the brief already in the conversation: no harness.
    let second = user_texts(&sim, 1).join("\n");
    assert!(second.contains("already in this conversation"), "{second}");
    assert_eq!(briefs_asked(&sim).len(), 1, "no second research");
    assert_eq!(source_chips(&sim), vec!["src/arena.rs:42".to_owned()]);
    assert_clean(&sim);
}

#[test]
fn more_detail_on_a_part_asks_for_its_component_brief() {
    let mut sim = session();
    share_and_read(&mut sim);
    sim.talker
        .push(ScriptedReply::says("Claude Code is reading the arena."));
    sim.talker.push(ScriptedReply::says(
        "The arena bumps a pointer inside the page.",
    ));
    sim.user_says(7_000, "More detail on the arena, please.");
    sim.run_until(20_000);
    assert_eq!(
        briefs_asked(&sim),
        vec![
            (SourceId(1), BriefTopic::Overview),
            (SourceId(1), BriefTopic::component("Arena"))
        ]
    );
    assert!(system_texts(&sim, 0).contains("is reading Arena of inf-alloc in more detail"));
    assert_eq!(
        sim.talker.requests.len(),
        2,
        "the brief gets a turn of its own"
    );
    let report = user_texts(&sim, 1).join("\n");
    assert!(
        report.contains("brief on the Arena of inf-alloc"),
        "{report}"
    );
    assert!(report.contains("# Brief: the arena"), "{report}");
    assert_clean(&sim);
}

#[test]
fn a_source_kept_in_the_project_is_there_next_session_with_its_cached_brief() {
    let mut first = session();
    share_and_read(&mut first);
    first.user_says(7_000, "Keep this in the project.");
    first.run_until(12_000);
    assert!(said(&first).contains(&"Kept in inf-alloc.".to_owned()));
    let project = last_tray(&first).project.expect("a project");
    assert!(!project.proposed && project.sources.len() == 1);
    // The next session: the runtime starts it on the same project, its vault and cache.
    let mut next = Sim::new(config());
    next.sources = first.sources.clone();
    opened(&mut next, Project::named("inf-alloc"));
    next.talker.push(ScriptedReply::says("One moment.").calls(
        "research",
        r#"{"question":"How does inf-alloc work?","depth":"quick"}"#,
    ));
    next.talker.push(ScriptedReply::says("It hands out slots."));
    next.run_until(1_000);
    let tray = last_tray(&next);
    let kept = tray.project.expect("the project").sources;
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(kept[0].title, "inf-alloc");
    next.user_says(2_000, "How does inf-alloc work?");
    next.run_until(10_000);
    let answered = user_texts(&next, 1).join("\n");
    assert!(
        answered.contains("from the cache, still fresh"),
        "{answered}"
    );
    assert!(
        answered.contains("# Brief: inf-alloc overview"),
        "{answered}"
    );
    assert!(
        next.harness.started.is_empty(),
        "no research for a fresh brief"
    );
    assert_clean(&next);
}

/// Source controls said in one breath with each other (`joined_cues`) are each answered by the
/// core; the talker hears none of it.
#[test]
fn keeping_and_the_tray_said_in_one_breath_are_both_answered() {
    let mut sim = session();
    share_and_read(&mut sim);
    let talks = sim.talker.requests.len();
    sim.user_says(7_000, "Show sources.");
    sim.run_until(8_000);
    assert!(last_tray(&sim).open);
    sim.user_says(9_000, "Keep this in the project, then hide sources.");
    sim.run_until(12_000);
    assert!(said(&sim).contains(&"Kept in inf-alloc.".to_owned()));
    let tray = last_tray(&sim);
    assert!(!tray.open, "hidden");
    let project = tray.project.expect("a project");
    assert!(!project.proposed && project.sources.len() == 1);
    assert_eq!(
        sim.talker.requests.len(),
        talks,
        "the talker heard none of it"
    );
    assert_clean(&sim);
}

#[test]
fn a_name_said_by_voice_is_resolved_or_asked_about() {
    let mut sim = session();
    sim.sources.folders = vec![
        code_folder(0, "inf-alloc", "crates/inf-alloc"),
        code_folder(0, "inf-alloc", "vendor/inf-alloc"),
        code_folder(0, "storage", "crates/storage"),
    ];
    sim.user_says(100, "Share the storage module.");
    sim.run_until(3_000);
    assert!(said(&sim).contains(&"Claude Code is reading storage.".to_owned()));
    sim.user_says(4_000, "Use the inf-alloc crate.");
    sim.run_until(7_000);
    assert!(
        said(&sim)
            .contains(&"I found 2: crates/inf-alloc and vendor/inf-alloc. Which one?".to_owned()),
        "{:?}",
        said(&sim)
    );
    sim.user_says(8_000, "The vendor one.");
    sim.run_until(11_000);
    let shared: Vec<SourceId> = source_effects(&sim)
        .into_iter()
        .filter_map(|effect| match effect {
            SourceEffect::Share { source } => Some(source),
            _ => None,
        })
        .collect();
    let rows = last_tray(&sim).session;
    let vendor = rows.iter().find(|row| row.path == "vendor/inf-alloc");
    assert!(
        vendor.is_some_and(|row| shared.contains(&row.id)),
        "{rows:?}"
    );
    assert!(!rows.iter().any(|row| row.path == "crates/inf-alloc"));
    sim.user_says(12_000, "Use the redis crate.");
    sim.run_until(15_000);
    assert!(said(&sim).contains(&"I couldn't find redis in your code folders.".to_owned()));
    assert!(
        sim.talker.requests.is_empty(),
        "the core answered every one itself"
    );
    assert_clean(&sim);
}

#[test]
fn sharing_while_boardai_speaks_never_cuts_the_voice() {
    let mut sim = session();
    sim.talker.push(ScriptedReply::says(
        "A cache sits in front of the database. Reads check it first, and a miss goes on to \
         the database, which fills the cache on the way back.",
    ));
    sim.user_says(0, "Explain a read-through cache.");
    assert!(sim.run_until_true(10_000, |sim| {
        *sim.core().state() == boardai_core::conversation::ConversationState::Speaking
    }));
    let at = sim.now().as_ms() + 200;
    sim.share(at, &code_folder(1, "inf-alloc", INF_ALLOC), false);
    sim.run_until(at + 8_000);
    let stops = sim
        .effects()
        .filter(|(when, effect)| {
            when.as_ms() >= at && matches!(effect, Effect::StopVoice | Effect::CancelTalk)
        })
        .count();
    assert_eq!(stops, 0, "the voice goes on");
    assert!(
        !said(&sim).contains(&"Claude Code is reading inf-alloc.".to_owned()),
        "BoardAI doesn't talk over itself"
    );
    assert_eq!(
        briefs_asked(&sim).len(),
        1,
        "the overview still starts at once"
    );
    assert_clean(&sim);
}

#[test]
fn a_brief_waits_while_you_draw_and_is_dropped_once_the_session_ended() {
    let mut sim = session();
    sim.sources.run_ms = 8_000;
    sim.share(100, &code_folder(1, "inf-alloc", INF_ALLOC), false);
    sim.talker.push(ScriptedReply::says("Let me see.").calls(
        "research",
        r#"{"question":"How does inf-alloc work?","depth":"quick"}"#,
    ));
    sim.talker
        .push(ScriptedReply::says("Claude Code is still reading it."));
    sim.talker.push(ScriptedReply::says("Here is the map."));
    sim.user_says(2_000, "How does inf-alloc work?");
    sim.run_until(5_000);
    let still = user_texts(&sim, 1).join("\n");
    assert!(still.contains("still reading"), "{still}");
    assert!(
        still.contains("<task-output>Reading the arena first.</task-output>"),
        "the harness's note, fenced: {still}"
    );
    sim.at(
        5_500,
        Input::Command(UiCommand::Draw {
            action: DrawAction::Start,
        }),
    );
    sim.run_until(12_000);
    let talks_while_drawing = sim.talker.requests.len();
    assert_eq!(
        talks_while_drawing, 2,
        "the brief arrived, and waits for the drawing (I14)"
    );
    sim.at(
        12_500,
        Input::Command(UiCommand::Draw {
            action: DrawAction::Cancel,
        }),
    );
    sim.run_until(20_000);
    assert_eq!(
        sim.talker.requests.len(),
        3,
        "it goes out once you stop drawing"
    );
    assert!(
        user_texts(&sim, 2)
            .join("\n")
            .contains("# Brief: inf-alloc overview")
    );
    assert_clean(&sim);

    let mut ended = session();
    ended.sources.run_ms = 8_000;
    ended.share(100, &code_folder(1, "inf-alloc", INF_ALLOC), false);
    // A running brief is a harness task: the first "end the session" warns (D-209), the second
    // stops it and ends the session; whatever it reports afterwards is dropped.
    ended.user_says(2_000, "End the session.");
    ended.user_says(5_000, "End the session.");
    ended.run_until(20_000);
    let end = ended
        .ui()
        .position(|(_, event)| matches!(event, UiEvent::SessionEnd))
        .expect("the session ended");
    let after: Vec<&UiEvent> = ended
        .ui()
        .skip(end + 1)
        .map(|(_, event)| event)
        .filter(|event| matches!(event, UiEvent::Sources { .. } | UiEvent::Chip { .. }))
        .collect();
    assert!(
        after.is_empty(),
        "nothing of the brief shows after the end: {after:?}"
    );
    assert!(ended.talker.requests.is_empty());
    assert_clean(&ended);
}

#[test]
fn a_document_waits_for_its_reader_and_its_digest_reaches_the_talker_fenced() {
    let mut sim = session();
    let paper = boardai_core::sources::SourceInfo {
        id: SourceId(7),
        kind: boardai_core::sources::SourceKind::Document {
            format: boardai_core::sources::DocumentFormat::Pdf,
        },
        title: "hekaton-sigmod13.pdf".into(),
        shown_path: "~/Papers/hekaton-sigmod13.pdf".into(),
        files_estimate: None,
    };
    sim.share(100, &paper, false);
    sim.run_until(1_000);
    assert!(
        briefs_asked(&sim).is_empty(),
        "documents are not code: no research"
    );
    assert!(matches!(
        &last_tray(&sim).session[0].state,
        SourceState::Waiting { reason } if reason == "Waiting for the document reader"
    ));
    let read = |state: SourceState, digest: Option<&str>| Input::Source {
        source: SourceInput::DocumentRead {
            source: SourceId(7),
            state,
            digest: digest.map(str::to_owned),
        },
    };
    sim.at(
        1_500,
        read(
            SourceState::Reading {
                reader: "BoardAI".into(),
                files_read: 3,
                files_estimate: Some(14),
            },
            None,
        ),
    );
    sim.at(
        2_500,
        read(
            SourceState::Partial {
                reason: "3 figures unread".into(),
            },
            Some("# Hekaton\n## Sections\n- Storage (p. 3)\n"),
        ),
    );
    sim.talker
        .push(ScriptedReply::says("Hekaton keeps rows in memory."));
    sim.user_says(3_000, "What is Hekaton about?");
    sim.run_until(8_000);
    assert!(trays(&sim).iter().any(|tray| matches!(
        tray.session[0].state,
        SourceState::Reading { files_read: 3, .. }
    )));
    assert!(matches!(
        &last_tray(&sim).session[0].state,
        SourceState::Partial { reason } if reason == "3 figures unread"
    ));
    let digest = user_texts(&sim, 0).join("\n");
    assert!(digest.contains("<task-output>\n# Hekaton"), "{digest}");
    assert_clean(&sim);
}
