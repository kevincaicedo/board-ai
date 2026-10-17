//! A seeded fuzzer over conversations with shared sources (review 3). A code folder and a paper
//! are shared; briefs come from the cache or from the harness, with diagrams. Random talker
//! replies read figures and pages, call `research` alone and in parallel (about the code, a
//! part, the paper), reveal staged parts and draw, while the user asks, drills into a part,
//! interrupts, stops, removes a source, shares it again, switches project and goes off the
//! record. Every step is checked (the simulator's invariants, D-107's system-role property and
//! I10 for sources), and nothing hangs: once the user stops acting, BoardAI settles.
#![allow(clippy::expect_used, reason = "a broken scenario should fail loudly")]
#![allow(
    clippy::wildcard_enum_match_arm,
    reason = "the coverage counts pick a few event kinds out of the whole trace"
)]

mod common;

use std::collections::BTreeMap;

use boardai_core::conversation::ConversationState;
use boardai_core::effect::Effect;
use boardai_core::input::Input;
use boardai_core::protocol::{DrawOp, Surface, UiCommand, UiEvent};
use boardai_core::sim::Sim;
use boardai_core::sim::sources::code_folder;
use boardai_core::sim::talker::ScriptedReply;
use boardai_core::sources::tray::SourceAction;
use boardai_core::sources::{BriefTopic, SourceEffect, SourceId, SourceInfo, SourceInput};
use common::config;
use common::documents::{DIGEST, PAPER, paper, read_event};

/// xorshift64*: a tiny deterministic generator, in test code only (the core never sees one).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound.max(1)
    }

    fn pick<'a>(&mut self, items: &'a [&'a str]) -> &'a str {
        let index = usize::try_from(self.below(items.len() as u64)).unwrap_or(0);
        items[index]
    }
}

const PATH: &str = "~/…/infinitydb/crates/inf-alloc";

/// The longest a reply may go on with nobody talking: the scripted harness and talker answer in
/// a few seconds, and a reply's tool rounds say a few sentences each (a minute at most, at the
/// turn's round limit), so anything longer is a call nobody answers.
const SETTLE_MAX_MS: u64 = 180_000;

fn settled(state: &ConversationState) -> bool {
    matches!(
        state,
        ConversationState::Idle | ConversationState::Muted | ConversationState::Listening(_)
    )
}

const OVERVIEW: &str = "# Brief: inf-alloc overview
## Overview
inf-alloc hands out memory from arenas.
## Components
- arena · Arena · type · src/arena.rs — Owns pages and hands out slots
- slot-pool · Slot pool · type · src/pool.rs — Recycles freed slots
## Diagram
<diagram title=\"inf-alloc\">
<node id=\"alloc\" kind=\"group\" label=\"inf-alloc\"/>
<node id=\"arena\" label=\"Arena\" in=\"alloc\"/>
<node id=\"slot-pool\" label=\"Slot pool\" in=\"alloc\"/>
<edge from=\"arena\" to=\"slot-pool\" label=\"frees\"/>
</diagram>
## Citations
- src/arena.rs:40-44
";

const ARENA: &str = "# Brief: the arena
## Overview
The arena keeps a free list per page.
## Components
- page · Page · type · src/arena.rs — 64 KiB of slots
## Diagram
<diagram title=\"Arena\">
<node id=\"page\" label=\"Page\"/>
<node id=\"free-list\" label=\"Free list\" kind=\"queue\"/>
<edge from=\"free-list\" to=\"page\" label=\"reuses\"/>
</diagram>
## Citations
- src/arena.rs:60-62
";

const FRAGMENTS: &[&str] = &[
    "Here is the map: <reveal id=\"alloc\" deep=\"true\"/>the allocator. ",
    "The <reveal id=\"arena\"/>arena hands out slots. ",
    "Each <reveal id=\"page\"/>page holds slots. ",
    "As Figure 1 (p. 3) shows, <reveal id=\"proxies\"/>proxies hand out versions. ",
    "Figure 2 shows the <reveal id=\"resolver\"/>resolver. ",
    "On page 5, recovery replaces the whole system. ",
    "Table 1 lists the latency. ",
    "It starts in the <node id=\"client\" label=\"Browser\" kind=\"actor\"/>browser, ",
    "then a <node id=\"free-list\" label=\"Free list\" near=\"arena\"/>free list takes it. ",
    "See src/arena.rs:40 for the slot. ",
    "A plain sentence with nothing drawn in it, long enough to take a while to say out loud. ",
];

const CALLS: &[(&str, &str)] = &[
    (
        "research",
        r#"{"question":"How does inf-alloc work?","depth":"quick"}"#,
    ),
    (
        "research",
        r#"{"question":"How does the Arena work?","depth":"quick"}"#,
    ),
    (
        "research",
        r#"{"question":"How does the Arena free a slot?","depth":"quick"}"#,
    ),
    (
        "research",
        r#"{"question":"What does the layered store paper say?","depth":"quick"}"#,
    ),
    ("read_file", r#"{"path":"layered-store.pdf#Figure 1"}"#),
    ("read_file", r#"{"path":"layered-store.pdf#Figure 2"}"#),
    (
        "read_file",
        r#"{"path":"layered-store.pdf","start_line":4,"end_line":7}"#,
    ),
    (
        "read_file",
        r#"{"path":"src/arena.rs","start_line":40,"end_line":44}"#,
    ),
];

const QUESTIONS: &[&str] = &[
    "How does inf-alloc work?",
    "Explain Figure 2 of the layered store paper.",
    "What does page 5 say?",
    "More detail on the arena.",
    "back out",
    "wait, why the arena?",
    "and then what happens",
];

fn reply(rng: &mut Rng) -> ScriptedReply {
    let mut text = String::new();
    for _ in 0..1 + rng.below(3) {
        text.push_str(rng.pick(FRAGMENTS));
    }
    let mut reply = ScriptedReply::says(&text);
    // Most replies call nothing; some call one tool, a few two at once (models call in parallel).
    for _ in 0..[0, 0, 1, 1, 2][usize::try_from(rng.below(5)).unwrap_or(0)] {
        let (name, input) = CALLS[usize::try_from(rng.below(CALLS.len() as u64)).unwrap_or(0)];
        reply = reply.calls(name, input);
    }
    reply
}

fn session(rng: &mut Rng) -> Sim {
    let mut sim = Sim::new(config());
    // Half the sessions stream fast: a provider's parallel calls arrive back to back, before
    // the runtime's cache answers the first.
    sim.talker.chunk_interval_ms = if rng.below(2) == 0 {
        1
    } else {
        1 + rng.below(200)
    };
    sim.talker.first_token_ms = 50 + rng.below(800);
    sim.sources.run_ms = 500 + rng.below(6_000);
    sim.sources.default_brief = OVERVIEW.to_owned();
    sim.sources
        .briefs
        .insert(BriefTopic::component("Arena").key(), ARENA.to_owned());
    if rng.below(2) == 0 {
        sim.sources.cache.insert(
            (PATH.to_owned(), BriefTopic::component("Arena").key()),
            ARENA.to_owned(),
        );
    }
    for (path, lines) in [("src/arena.rs", 200), ("src/pool.rs", 80)] {
        sim.sources.readable.insert(path.into(), lines);
    }
    sim.sources.pages.insert(
        PAPER,
        (1..=7)
            .map(|page| format!("Page {page} says recovery replaces the whole system."))
            .collect(),
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
    sim.share(100, &code_folder(1, "inf-alloc", PATH), rng.below(3) == 0);
    sim.share(200, &paper(), false);
    sim.at(700, read_event(DIGEST));
    for _ in 0..40 {
        let reply = reply(rng);
        sim.talker.push(reply);
    }
    sim
}

/// One random user action now; returns how long to leave the floor alone.
fn act(sim: &mut Sim, rng: &mut Rng, shared_again: &mut u32) -> u64 {
    let now = sim.now().as_ms();
    let remove = |source: SourceId| {
        Input::Command(UiCommand::Sources {
            action: SourceAction::Remove { source },
        })
    };
    match rng.below(14) {
        0..=5 => {
            let question = rng.pick(QUESTIONS);
            sim.user_says(now, question);
            2_500
        }
        6 | 7 => {
            sim.at(now, Input::Command(UiCommand::Stop));
            0
        }
        8 => {
            let source = if rng.below(2) == 0 {
                SourceId(1)
            } else {
                PAPER
            };
            sim.at(now, remove(source));
            0
        }
        9 => {
            // Shared again under a new id: the runtime never reuses one in a run.
            *shared_again += 1;
            let info = SourceInfo {
                id: SourceId(20 + *shared_again),
                ..code_folder(1, "inf-alloc", PATH)
            };
            sim.share(now, &info, false);
            0
        }
        10 => {
            sim.user_says(now, "We're working on infinitydb.");
            2_000
        }
        11 => {
            sim.user_says(now, "Don't remember this session.");
            2_000
        }
        12 => {
            sim.user_says(now, "Keep this in the project.");
            2_000
        }
        _ => 0,
    }
}

fn run_seed(seed: u64, coverage: &mut BTreeMap<&'static str, usize>) {
    let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
    let mut sim = session(&mut rng);
    sim.user_says(1_000, "How does inf-alloc work?");
    let mut shared_again = 0;
    for _ in 0..20 {
        let quiet = act(&mut sim, &mut rng, &mut shared_again);
        let until = sim.now().as_ms() + quiet.max(100 + rng.below(5_000));
        sim.run_until(until);
        if rng.below(3) == 0 {
            // Left alone, a reply always ends: a call held for ever would keep it going (the
            // voice done, the reply never finished).
            let limit = sim.now().as_ms() + SETTLE_MAX_MS;
            let ended = settled(sim.core().state())
                || sim.run_until_true(limit, |sim| settled(sim.core().state()));
            assert!(
                ended,
                "seed {seed}: still {} after {SETTLE_MAX_MS} ms with nobody talking",
                sim.core().state().name()
            );
        }
    }
    // The user stops acting: BoardAI finishes and settles, never waiting on a call for ever.
    let end = sim.now().as_ms() + 120_000;
    sim.run_until(end);
    assert!(
        sim.violations().is_empty(),
        "seed {seed}: {:#?}",
        sim.violations()
    );
    assert!(
        settled(sim.core().state()),
        "seed {seed} ended in {}",
        sim.core().state().name()
    );
    count(&sim, coverage);
}

fn count(sim: &Sim, coverage: &mut BTreeMap<&'static str, usize>) {
    for (_, effect) in sim.effects() {
        let key = match effect {
            Effect::Source {
                source: SourceEffect::Brief { .. },
            } => "brief",
            Effect::Source {
                source: SourceEffect::Discard { .. },
            } => "discard",
            Effect::Source {
                source: SourceEffect::ReadDocument { .. },
            } => "page read",
            Effect::CancelHarness { .. } => "cancel",
            Effect::Forget { .. } => "off record",
            Effect::Ui(UiEvent::Diagram { part: 0, .. }) => "staged",
            Effect::Ui(UiEvent::OpStart {
                op: DrawOp::Reveal { .. },
                ..
            }) => "reveal",
            Effect::Ui(UiEvent::SurfaceShow {
                surface: Surface::Doc { .. },
            }) => "doc card",
            Effect::Ui(UiEvent::LayerPush { .. }) => "detail",
            Effect::StopVoice => "stop",
            _ => continue,
        };
        *coverage.entry(key).or_default() += 1;
    }
    let deferred = (0..sim.talker.requests.len()).any(|index| {
        common::system_texts(sim, index).contains("once the words you already wrote have played")
    });
    if deferred {
        *coverage.entry("deferred staging").or_default() += 1;
    }
}

#[test]
fn random_conversations_with_sources_keep_every_invariant() {
    let mut coverage = BTreeMap::new();
    for seed in 1..=120 {
        run_seed(seed, &mut coverage);
    }
    for key in [
        "brief",
        "discard",
        "page read",
        "cancel",
        "off record",
        "staged",
        "reveal",
        "doc card",
        "detail",
        "stop",
        "deferred staging",
    ] {
        assert!(
            coverage.get(key).is_some_and(|count| *count > 0),
            "never exercised: {key} ({coverage:?})"
        );
    }
}
