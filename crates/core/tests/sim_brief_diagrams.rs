//! A research brief's diagram on the board (D-242): read with the narration's parser as the
//! brief arrives, staged when the brief reaches the talker (an overview on the board, a
//! component brief on its part's detail layer), revealed as the talker explains it and the rest
//! as its reply ends, with where it came from on the header. Harness text stays data: what
//! doesn't parse or fit is dropped and counted, and the brief is still used. A brief that
//! arrives while you draw waits for you (I14). Every run also holds I1, I6, I11 and I16.
#![allow(clippy::expect_used, reason = "a broken scenario should fail loudly")]
#![allow(
    clippy::wildcard_enum_match_arm,
    reason = "scenarios pick one event kind out of the whole trace"
)]

mod common;

use boardai_core::effect::{Dropped, Effect, LogRecord};
use boardai_core::geometry::Rect;
use boardai_core::history::{Message, UserPart};
use boardai_core::ids::{ElementId, LayerId};
use boardai_core::input::Input;
use boardai_core::protocol::{
    DiagramProvenance, DrawAction, DrawOp, DrawnShape, UiCommand, UiEvent,
};
use boardai_core::sim::Sim;
use boardai_core::sim::sources::code_folder;
use boardai_core::sim::talker::ScriptedReply;
use boardai_core::sources::{BriefTopic, SourceInput};
use boardai_core::{CoreConfig, RepoReads};
use common::{assert_clean, config, count};

const INF_ALLOC: &str = "~/…/infinitydb/crates/inf-alloc";

/// An overview whose diagram maps three parts in a container, with two connectors.
const OVERVIEW: &str = "# Brief: inf-alloc overview
## Overview
inf-alloc hands out memory from arenas. Each arena carves slots from aligned pages.
## Components
- arena · Arena · type · src/arena.rs — Owns pages and hands out slots
- slot-pool · Slot pool · module · src/pool/slot.rs — Recycles freed slots
- pages · Pages · module · src/pages.rs — Aligned 4 KiB pages
## Relations
- arena → slot-pool: returns freed slots
- pages → arena: backs
## Diagram
<diagram title=\"inf-alloc · memory\">
<node id=\"alloc\" kind=\"group\" label=\"inf-alloc\"/>
<node id=\"arena\" label=\"Arena\" kind=\"service\" in=\"alloc\"/>
<node id=\"slot-pool\" label=\"Slot pool\" kind=\"service\" in=\"alloc\"/>
<node id=\"pages\" label=\"Pages\" kind=\"db\" in=\"alloc\"/>
<edge from=\"arena\" to=\"slot-pool\" label=\"frees\"/>
<edge from=\"pages\" to=\"arena\" label=\"backs\"/>
</diagram>
## Citations
- src/arena.rs:40-44
";

/// The arena in more detail, with the diagram of its inside.
const ARENA: &str = "# Brief: the arena
## Overview
The arena keeps a free list per page and bumps a pointer inside the page.
## Components
- page · Page · type · src/arena.rs — 64 KiB of slots
- free-list · Free list · type · src/arena.rs — Freed slots, newest first
## Diagram
<diagram title=\"Arena · inside\">
<node id=\"page\" label=\"Page\" kind=\"service\"/>
<node id=\"free-list\" label=\"Free list\" kind=\"queue\"/>
<edge from=\"free-list\" to=\"page\" label=\"reuses\"/>
</diagram>
## Citations
- src/arena.rs:60-62
";

/// A brief whose diagram mixes good parts with harness text that doesn't fit the vocabulary: a
/// sequence diagram's tag (misplaced in a block), a tag nobody knows, an id that isn't one, an
/// id written twice and a connector to nothing.
const MALFORMED: &str = "# Brief: inf-alloc overview
## Overview
inf-alloc hands out memory from arenas.
## Components
- arena · Arena · type · src/arena.rs — Owns pages
- slot-pool · Slot pool · module · src/pool/slot.rs — Recycles freed slots
## Diagram
<diagram title=\"inf-alloc\">
<node id=\"arena\" label=\"Arena\"/>
<participant id=\"p\" label=\"Caller\"/>
<frobnicate level=\"9\"/>
<node id=\"Not An Id!\" label=\"Bad\"/>
<node id=\"arena\" label=\"Arena again\"/>
<node id=\"slot-pool\" label=\"Slot pool\"/>
<edge from=\"arena\" to=\"ghost\"/>
<edge from=\"arena\" to=\"slot-pool\"/>
</diagram>
## Citations
- src/arena.rs:40-44
";

/// A session with Claude Code installed and a scripted runtime that knows inf-alloc's files.
fn session(overview: &str) -> Sim {
    let mut sim = Sim::new(config());
    sim.sources.default_brief = overview.to_owned();
    sim.sources
        .briefs
        .insert(BriefTopic::component("Arena").key(), ARENA.to_owned());
    sim.sources.touched = vec!["src/arena.rs".into(), "src/pool/slot.rs".into()];
    sim.sources.readable.insert("src/arena.rs".into(), 200);
    sim.sources.readable.insert("src/pool/slot.rs".into(), 80);
    sim.at(
        0,
        Input::Source {
            source: SourceInput::Opened {
                harness: Some("Claude Code".into()),
                project: None,
            },
        },
    );
    sim
}

/// Shares inf-alloc and lets its overview arrive, quietly: nobody asked yet.
fn share_and_read(sim: &mut Sim) {
    sim.share(100, &code_folder(1, "inf-alloc", INF_ALLOC), false);
    sim.run_until(6_000);
}

/// The talker asks for the brief it already has, as the shared notice tells it to.
fn uses_the_brief() -> ScriptedReply {
    ScriptedReply::says("Let me check the brief.").calls(
        "research",
        r#"{"question":"How does inf-alloc hand out memory?","depth":"quick"}"#,
    )
}

/// Each staged diagram's header events: layer, part, and where it came from.
fn headers(sim: &Sim) -> Vec<(LayerId, u8, Option<DiagramProvenance>)> {
    sim.ui()
        .filter_map(|(_, event)| match event {
            UiEvent::Diagram {
                layer,
                part,
                provenance,
                ..
            } => Some((*layer, *part, provenance.clone())),
            _ => None,
        })
        .collect()
}

/// Each reveal's layer and parts, in order.
fn reveals(sim: &Sim) -> Vec<(LayerId, Vec<String>)> {
    sim.ui()
        .filter_map(|(_, event)| match event {
            UiEvent::OpStart {
                layer,
                op: DrawOp::Reveal { parts, .. },
                ..
            } => Some((
                *layer,
                parts
                    .iter()
                    .map(|part| part.id().as_str().to_owned())
                    .collect(),
            )),
            _ => None,
        })
        .collect()
}

fn shown(sim: &Sim, layer: LayerId, id: &str) -> bool {
    let id = ElementId::parse(id).expect("id");
    sim.core()
        .board()
        .layer(layer)
        .and_then(|layer| layer.element(&id))
        .is_some_and(|element| element.state.is_shown())
}

fn system_text(sim: &Sim, index: usize) -> String {
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

fn user_text(sim: &Sim, index: usize) -> String {
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
        .collect::<Vec<_>>()
        .join("\n")
}

fn dropped(sim: &Sim, what: Dropped) -> u32 {
    sim.effects()
        .filter_map(|(_, effect)| match effect {
            Effect::Log(LogRecord::Dropped { what: kind, count }) if *kind == what => Some(*count),
            _ => None,
        })
        .sum()
}

/// The overview's diagram waits for the talker to use the brief (a prefetch went quietly with
/// the question), then is staged with the request that carries the answer: nothing drawn, the
/// header naming where it came from, and a note with its ids. The talker reveals two parts as
/// it explains them; the third, with its connector, is drawn as the reply ends.
#[test]
fn a_briefs_diagram_is_staged_when_used_and_revealed_as_it_is_explained() {
    let mut sim = session(OVERVIEW);
    share_and_read(&mut sim);
    sim.talker.push(uses_the_brief());
    sim.talker.push(ScriptedReply::says(concat!(
        "Memory comes from <reveal id=\"alloc\"/>inside inf-alloc. The <reveal id=\"arena\"/>",
        "arena hands out slots, and freed ones go back to the <reveal id=\"slot-pool\"/>slot ",
        "pool, which recycles them."
    )));
    sim.user_says(7_000, "How does inf-alloc hand out memory?");
    sim.run_until(40_000);
    assert_clean(&sim);
    assert!(
        !system_text(&sim, 0).contains("is staged on the board"),
        "nothing staged before the talker used the brief"
    );
    let note = system_text(&sim, 1);
    assert!(
        note.contains(
            "The diagram of Claude Code's overview brief of inf-alloc is staged on the board \
             (id diagram); nothing of it is drawn yet."
        ),
        "{note}"
    );
    // By id alone: its title and labels are the brief's words, which the talker reads between
    // the task-output marks (D-265).
    assert!(
        note.contains(
            "Its parts, by id (their words are in the fenced text it came with): \
                       alloc, arena, slot-pool, pages."
        ),
        "{note}"
    );
    assert!(
        !note.contains("Slot pool") && !note.contains("inf-alloc · memory"),
        "{note}"
    );
    assert!(user_text(&sim, 1).contains("BoardAI stages its diagram on the board now"));
    let from = Some(DiagramProvenance {
        reader: "Claude Code".into(),
        source: "inf-alloc".into(),
        files_read: Some(2),
    });
    let headers = headers(&sim);
    assert_eq!(headers.first(), Some(&(LayerId::MAIN, 0, from.clone())));
    assert!(
        headers.iter().all(|(_, _, provenance)| *provenance == from),
        "every header names the brief: {headers:?}"
    );
    let reveals = reveals(&sim);
    let order: Vec<&str> = reveals
        .iter()
        .flat_map(|(_, parts)| parts.iter().map(String::as_str))
        .collect();
    assert_eq!(&order[..3], ["alloc", "arena", "slot-pool"], "{order:?}");
    assert_eq!(
        reveals.last().map(|(_, parts)| parts.clone()),
        Some(vec!["pages".to_owned(), "pages>arena".to_owned()]),
        "the part the talker left, with its connector, as the reply ends"
    );
    for part in ["alloc", "arena", "slot-pool", "pages", "arena>slot-pool"] {
        assert!(shown(&sim, LayerId::MAIN, part), "{part}");
    }
}

/// A brief the user asked for gets a report turn, and its diagram goes with it. A talker that
/// reveals nothing still shows the whole map: it is drawn as the reply ends.
#[test]
fn a_reported_brief_is_staged_with_its_turn_and_drawn_whole_when_nothing_reveals_it() {
    let mut sim = session(OVERVIEW);
    sim.talker.push(ScriptedReply::says("Let me look.").calls(
        "research",
        r#"{"question":"How does inf-alloc work?","depth":"quick"}"#,
    ));
    sim.talker
        .push(ScriptedReply::says("Claude Code is reading it now."));
    sim.talker.push(ScriptedReply::says(
        "Claude Code mapped it: an arena, a slot pool and pages.",
    ));
    sim.share(100, &code_folder(1, "inf-alloc", INF_ALLOC), false);
    sim.user_says(500, "How does inf-alloc work?");
    sim.run_until(40_000);
    assert_clean(&sim);
    assert_eq!(
        sim.talker.requests.len(),
        3,
        "the brief had a turn of its own"
    );
    assert!(system_text(&sim, 2).contains("is staged on the board"));
    let drawn: Vec<String> = reveals(&sim)
        .into_iter()
        .flat_map(|(_, parts)| parts)
        .collect();
    assert_eq!(drawn.len(), 6, "every part and connector, once: {drawn:?}");
}

/// "More detail on the arena": its detail layer opens at once and the talker is told the
/// brief's diagram will be staged there, so it draws no guess; the component brief's diagram is
/// staged on that layer when the brief arrives, and revealed there.
#[test]
fn a_component_briefs_diagram_is_staged_on_its_detail_layer() {
    let mut sim = session(OVERVIEW);
    share_and_read(&mut sim);
    sim.talker.push(uses_the_brief());
    sim.talker.push(ScriptedReply::says(
        "Here is the map: <reveal id=\"alloc\" deep=\"true\"/>the arena, the slot pool and pages.",
    ));
    sim.user_says(7_000, "How does inf-alloc hand out memory?");
    sim.run_until(30_000);
    sim.talker
        .push(ScriptedReply::says("Claude Code is reading the arena."));
    sim.talker.push(ScriptedReply::says(concat!(
        "Inside, each <reveal id=\"page\"/>page holds slots, and the ",
        "<reveal id=\"free-list\"/>free list hands back the newest freed one."
    )));
    sim.user_says(30_000, "More detail on the arena.");
    sim.run_until(60_000);
    assert_clean(&sim);
    let pushed: Vec<(LayerId, String)> = sim
        .ui()
        .filter_map(|(_, event)| match event {
            UiEvent::LayerPush {
                layer,
                anchor: Some(anchor),
                ..
            } => Some((*layer, anchor.element.as_str().to_owned())),
            _ => None,
        })
        .collect();
    assert_eq!(pushed, [(LayerId(1), "arena".to_owned())]);
    let asked = system_text(&sim, 2);
    assert!(
        asked.contains("don't draw its internals yourself"),
        "{asked}"
    );
    // Review 3 hygiene (red at 2b76809): the note had lost its line continuations and carried
    // runs of spaces.
    assert!(!asked.contains("   "), "{asked}");
    let note = system_text(&sim, 3);
    assert!(
        note.contains("on the detail layer anchored to arena, open in front of the board"),
        "{note}"
    );
    let detail: Vec<(LayerId, u8, Option<DiagramProvenance>)> = headers(&sim)
        .into_iter()
        .filter(|(layer, _, _)| *layer == LayerId(1))
        .collect();
    assert_eq!(detail.first().map(|(_, part, _)| *part), Some(0));
    assert!(
        detail
            .iter()
            .all(|(_, _, from)| from.as_ref().is_some_and(|from| from.files_read == Some(2))),
        "{detail:?}"
    );
    for part in ["page", "free-list", "free-list>page"] {
        assert!(shown(&sim, LayerId(1), part), "{part}");
    }
}

/// A component brief the talker asked for by name, with no detail layer open: the part is on
/// the board, so its detail layer opens for the diagram (D-238), within the detour depth.
#[test]
fn a_component_brief_opens_its_parts_detail_layer() {
    let mut sim = session(OVERVIEW);
    share_and_read(&mut sim);
    sim.talker.push(uses_the_brief());
    sim.talker.push(ScriptedReply::says(
        "Here: <reveal id=\"alloc\" deep=\"true\"/>the whole allocator.",
    ));
    sim.user_says(7_000, "How does inf-alloc hand out memory?");
    sim.run_until(30_000);
    sim.talker
        .push(ScriptedReply::says("Let me ask about the arena.").calls(
            "research",
            r#"{"question":"How does the Arena reuse slots?","depth":"quick"}"#,
        ));
    sim.talker
        .push(ScriptedReply::says("Claude Code is reading the arena."));
    sim.talker.push(ScriptedReply::says(
        "Each page holds slots; the free list reuses them.",
    ));
    sim.user_says(30_000, "How does the arena reuse slots?");
    sim.run_until(60_000);
    assert_clean(&sim);
    let pushed = count(&sim, |effect| {
        matches!(
            effect,
            Effect::Ui(UiEvent::LayerPush {
                anchor: Some(_),
                ..
            })
        )
    });
    assert_eq!(pushed, 1);
    assert!(shown(&sim, LayerId(1), "page"), "drawn as the reply ended");
    assert!(shown(&sim, LayerId(1), "free-list"));
}

/// Harness text is data: tags that don't fit the vocabulary are dropped and counted, parts that
/// don't fit the board too, and the rest is staged; the brief still reaches the talker, told
/// how many went. A diagram with nothing readable stages nothing, and the talker is told to
/// draw with its own tags.
#[test]
fn a_malformed_diagram_in_a_brief_is_dropped_and_counted_and_the_brief_still_used() {
    let mut sim = session(MALFORMED);
    share_and_read(&mut sim);
    sim.talker.push(uses_the_brief());
    sim.talker.push(ScriptedReply::says(
        "The <reveal id=\"arena\"/>arena hands slots to the <reveal id=\"slot-pool\"/>slot pool.",
    ));
    sim.user_says(7_000, "How does inf-alloc hand out memory?");
    sim.run_until(40_000);
    assert_clean(&sim);
    // In the block: the participant, the unknown tag and the bad id; in staging: the repeated
    // id and the connector to nothing.
    let parts = dropped(&sim, Dropped::DiagramPart);
    assert!(parts >= 5, "{parts} dropped");
    let brief = user_text(&sim, 0);
    assert!(brief.contains("# Brief: inf-alloc overview"), "{brief}");
    let note = system_text(&sim, 1);
    assert!(
        note.contains("5 of its tags or parts were dropped"),
        "{note}"
    );
    for part in ["arena", "slot-pool", "arena>slot-pool"] {
        assert!(shown(&sim, LayerId::MAIN, part), "{part}");
    }

    let unreadable = OVERVIEW.replace(
        &OVERVIEW[OVERVIEW.find("<diagram").expect("a diagram")
            ..OVERVIEW.find("## Citations").expect("citations")],
        "<diagram title=\"x\">\n<participant id=\"p\" label=\"P\"/>\n</diagram>\n",
    );
    let mut sim = session(&unreadable);
    share_and_read(&mut sim);
    sim.talker.push(uses_the_brief());
    sim.talker.push(ScriptedReply::says("It hands out slots."));
    sim.user_says(7_000, "How does inf-alloc hand out memory?");
    sim.run_until(30_000);
    assert_clean(&sim);
    assert!(headers(&sim).is_empty(), "nothing staged");
    assert!(user_text(&sim, 0).contains("Its diagram couldn't be read"));
}

/// A brief that arrives while you draw waits for you (I14): nothing is staged under your pen.
/// Once you finish, its report turn stages the diagram clear of what you drew, and it is drawn.
#[test]
fn a_brief_arriving_while_you_draw_is_staged_once_you_finish() {
    let mut sim = session(OVERVIEW);
    sim.sources.run_ms = 8_000;
    sim.share(100, &code_folder(1, "inf-alloc", INF_ALLOC), false);
    sim.talker.push(ScriptedReply::says("Let me see.").calls(
        "research",
        r#"{"question":"How does inf-alloc work?","depth":"quick"}"#,
    ));
    sim.talker
        .push(ScriptedReply::says("Claude Code is still reading it."));
    sim.talker.push(ScriptedReply::says(
        "Here it is: <reveal id=\"alloc\" deep=\"true\"/>an arena, a slot pool and pages.",
    ));
    sim.user_says(2_000, "How does inf-alloc work?");
    sim.run_until(5_000);
    let draw = |action| Input::Command(UiCommand::Draw { action });
    sim.at(5_500, draw(DrawAction::Start));
    let yours = Rect {
        x: 0,
        y: 0,
        w: 400,
        h: 300,
    };
    sim.at(
        6_000,
        draw(DrawAction::Shape {
            layer: LayerId::MAIN,
            drawn: DrawnShape::Box { rect: yours },
        }),
    );
    sim.run_until(12_000);
    assert!(headers(&sim).is_empty(), "nothing staged while you draw");
    assert_eq!(sim.talker.requests.len(), 2);
    sim.at(12_500, draw(DrawAction::Done));
    sim.run_until(40_000);
    assert_clean(&sim);
    let staged_at = sim
        .ui()
        .find(|(_, event)| matches!(event, UiEvent::Diagram { part: 0, .. }))
        .map(|(at, _)| at.as_ms())
        .expect("staged");
    assert!(staged_at >= 12_500, "staged at {staged_at} ms");
    let staged = &sim.core().board().staged(LayerId::MAIN)[0];
    assert!(!staged.bounds.intersects(yours), "clear of your box");
    assert!(shown(&sim, LayerId::MAIN, "pages"));
}

/// A map too big for a page with drawing on it turns the page first (D-177), and the talker is
/// told.
#[test]
fn a_brief_diagram_too_big_for_the_page_turns_it() {
    let mut nodes = String::new();
    for index in 0..40 {
        nodes.push_str(&format!(
            "<node id=\"n{index}\" label=\"module number {index}\" kind=\"service\"/>\n"
        ));
    }
    let wide = OVERVIEW.replace(
        &OVERVIEW[OVERVIEW.find("<diagram").expect("a diagram")
            ..OVERVIEW.find("## Citations").expect("citations")],
        &format!("<diagram title=\"tall\" layout=\"column\">\n{nodes}</diagram>\n"),
    );
    let mut sim = session(&wide);
    share_and_read(&mut sim);
    sim.talker.push(ScriptedReply::says(
        "A cache <node id=\"c\" label=\"Cache\"/>sits in front.",
    ));
    sim.user_says(7_000, "Sketch a cache for me.");
    sim.run_until(20_000);
    sim.talker.push(uses_the_brief());
    sim.talker.push(ScriptedReply::says(
        "It has forty modules, one under another.",
    ));
    sim.user_says(20_000, "How does inf-alloc hand out memory?");
    sim.run_until(60_000);
    assert_clean(&sim);
    let turned = count(&sim, |effect| {
        matches!(effect, Effect::Ui(UiEvent::PageTurn { .. }))
    });
    assert_eq!(turned, 1);
    assert!(system_text(&sim, 2).contains("The page turned to make room for it."));
    assert!(shown(&sim, LayerId::MAIN, "n39"));
}

/// D-232's retry needs the repo reads offered. A session with no code folder offers none until
/// a code source is shared: the runtime then says so (`RepoReads`), and an announced read is
/// asked for as a call.
#[test]
fn the_repo_reads_turn_on_when_a_code_source_is_shared() {
    const ANNOUNCES: &str = "I will start by listing the files in the folder.";
    const ASKED: &str = "said it would look at the repo but called no tool";
    let mut sim = Sim::new(CoreConfig {
        repo_reads: RepoReads::NotOffered,
        ..config()
    });
    sim.talker.push(ScriptedReply::says(ANNOUNCES));
    sim.user_says(0, "Walk me through this repo.");
    sim.run_until(15_000);
    assert_eq!(sim.talker.requests.len(), 1, "nothing to call: no retry");
    sim.at(
        15_000,
        Input::Source {
            source: SourceInput::RepoReads {
                reads: RepoReads::Offered,
            },
        },
    );
    sim.talker.push(ScriptedReply::says(ANNOUNCES));
    sim.talker.push(ScriptedReply::says("Listing it now."));
    sim.user_says(16_000, "Walk me through the shared folder.");
    sim.run_until(40_000);
    assert_clean(&sim);
    assert_eq!(sim.talker.requests.len(), 3, "asked once more for the call");
    let asked = format!("{:?}", sim.talker.requests[2].messages);
    assert!(asked.contains(ASKED), "{asked}");
}

/// An overview whose diagram labels one file the brief cited and one it didn't.
const NAMES_FILES: &str = "# Brief: inf-alloc overview
## Overview
inf-alloc hands out memory from arenas.
## Components
- arena · Arena · type · src/arena.rs — Owns pages
## Diagram
<diagram title=\"inf-alloc · files\">
<node id=\"alloc\" kind=\"group\" label=\"inf-alloc\"/>
<node id=\"arena-file\" label=\"src/arena.rs\" in=\"alloc\"/>
<node id=\"heap-file\" label=\"src/heap.rs\" in=\"alloc\"/>
</diagram>
## Citations
- src/arena.rs:40-44
";

/// Each reveal's parts and budget, in order.
fn budgets(sim: &Sim) -> Vec<(usize, u32)> {
    sim.ui()
        .filter_map(|(_, event)| match event {
            UiEvent::OpStart {
                op: DrawOp::Reveal { parts, .. },
                budget_ms,
                ..
            } => Some((parts.len(), *budget_ms)),
            _ => None,
        })
        .collect()
}

/// A brief's leftovers take the one leftover path (D-250), as the talker's own diagrams do: once
/// everything said has played, at a pace per part.
#[test]
fn a_briefs_leftovers_are_drawn_at_the_leftover_pace() {
    let mut sim = session(OVERVIEW);
    share_and_read(&mut sim);
    sim.talker.push(uses_the_brief());
    sim.talker.push(ScriptedReply::says(
        "Memory comes from <reveal id=\"alloc\"/>inside inf-alloc, where an arena hands it out.",
    ));
    sim.user_says(7_000, "How does inf-alloc hand out memory?");
    sim.run_until(40_000);
    assert_clean(&sim);
    let budgets = budgets(&sim);
    let (parts, budget_ms) = *budgets.last().expect("the leftovers' reveal");
    assert_eq!(
        budgets.len(),
        2,
        "the named part, then the rest: {budgets:?}"
    );
    let parts = u32::try_from(parts).expect("a count");
    assert_eq!(
        budget_ms,
        parts * boardai_core::limits::LEFTOVER_REVEAL_MS_PER_PART,
        "{budgets:?}"
    );
    assert!(shown(&sim, LayerId::MAIN, "pages"));
}

/// The one interruption rule (D-250): an interrupted reply reveals none of what it left, and the
/// next turn's reply doesn't either; the summary names what is still planned.
#[test]
fn an_interrupted_reply_leaves_a_briefs_parts_planned() {
    let mut sim = session(OVERVIEW);
    share_and_read(&mut sim);
    sim.talker.push(uses_the_brief());
    sim.talker.push(ScriptedReply::says(concat!(
        "Memory comes from <reveal id=\"alloc\"/>inside inf-alloc. The arena hands out slots ",
        "from aligned pages, and a freed slot goes back to a pool that recycles it, so the next ",
        "allocation of that size is quick and never touches the operating system at all."
    )));
    sim.talker.push(ScriptedReply::says(
        "Because pages are aligned to four kilobytes.",
    ));
    sim.user_says(7_000, "How does inf-alloc hand out memory?");
    assert!(sim.run_until_true(40_000, |sim| !reveals(sim).is_empty()));
    let at = sim.now().as_ms() + 100;
    sim.user_says_with_onset(at, "wait, why pages?");
    sim.run_until(at + 30_000);
    assert_clean(&sim);
    assert!(!shown(&sim, LayerId::MAIN, "pages"), "{:?}", reveals(&sim));
    assert!(!shown(&sim, LayerId::MAIN, "slot-pool"));
}

/// A brief's diagram is harness output, held to what the core read of it (D-252): a label naming
/// a file none of the brief's checked citations names is marked "?" in place, and the note says
/// so; the file it cited, and labels in words, are left alone.
#[test]
fn a_briefs_diagram_marks_files_its_checked_citations_dont_name() {
    let mut sim = session(NAMES_FILES);
    share_and_read(&mut sim);
    sim.talker.push(uses_the_brief());
    sim.talker.push(ScriptedReply::says(
        "The allocator <reveal id=\"alloc\" deep=\"true\"/>has files.",
    ));
    sim.user_says(7_000, "How does inf-alloc hand out memory?");
    sim.run_until(40_000);
    assert_clean(&sim);
    let note = system_text(&sim, 1);
    assert!(
        note.contains(
            "1 part was marked \"?\": the brief's checked citations name none of their paths \
             (heap-file)."
        ),
        "{note}"
    );
    let drawn = format!("{:?}", sim.ui().collect::<Vec<_>>());
    assert!(drawn.contains("\"src/heap.rs ?\""), "{drawn}");
    assert!(drawn.contains("\"src/arena.rs\""), "{drawn}");
    assert!(drawn.contains("\"inf-alloc\""), "{drawn}");
}
