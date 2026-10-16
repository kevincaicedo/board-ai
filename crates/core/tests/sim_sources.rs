//! Source chips (PRODUCT §7.3, D-152): a `path:line` the talker says shows a chip carrying the
//! location when that line was read this session, and clicking it opens the code card on that
//! line. Nothing unread ever gets a chip or a card (law 6).
#![allow(clippy::expect_used, reason = "a broken scenario should fail loudly")]
#![allow(
    clippy::wildcard_enum_match_arm,
    reason = "scenarios pick one event kind out of the whole trace"
)]

mod common;

use boardai_core::input::Input;
use boardai_core::protocol::{ChipKind, SourceRef, Surface, SurfaceUpdate, UiCommand, UiEvent};
use boardai_core::sim::Sim;
use boardai_core::sim::talker::ScriptedReply;
use boardai_core::tools::ToolOutput;
use common::{assert_clean, config, dropped};

const PATH: &str = "internal/user/service.go";

fn chips(sim: &Sim) -> Vec<(u64, String, Option<SourceRef>)> {
    sim.ui()
        .filter_map(|(at, event)| match event {
            UiEvent::Chip {
                kind: ChipKind::Source,
                text,
                source,
            } => Some((at.as_ms(), text.clone(), source.clone())),
            _ => None,
        })
        .collect()
}

fn reading(sim: &mut Sim) {
    let lines = (40..=60).map(|line| format!("// line {line}")).collect();
    sim.tools.respond(
        "read_file",
        ToolOutput::File {
            path: PATH.into(),
            first_line: 40,
            lines,
            more_lines: 0,
        },
    );
    sim.talker
        .push(ScriptedReply::says("Let me read it.").calls(
            "read_file",
            r#"{"path":"internal/user/service.go","start_line":40,"end_line":60}"#,
        ));
}

#[test]
fn a_cited_line_that_was_read_shows_one_chip_as_it_is_said() {
    let mut sim = Sim::new(config());
    reading(&mut sim);
    sim.talker.push(ScriptedReply::says(
        "The lookup in service.go:43 asks Redis first. Again, service.go:43 is the line. \
         The handler in internal/api/handler.go:12 calls it, and internal/user/service.go:90 \
         was never read.",
    ));
    sim.user_says(0, "Where do we look users up?");
    sim.run_until(60_000);
    assert_clean(&sim);
    let chips = chips(&sim);
    assert_eq!(
        chips
            .iter()
            .map(|(_, text, source)| (text.as_str(), source.clone()))
            .collect::<Vec<_>>(),
        [(
            "internal/user/service.go:43",
            Some(SourceRef {
                path: PATH.into(),
                line: 43
            })
        )],
        "once per reply, only for a read line, with the path it was read with"
    );
    let caption = |words: &str| {
        sim.ui()
            .find_map(|(at, event)| match event {
                UiEvent::Caption { text, .. } if text.contains(words) => Some(at.as_ms()),
                _ => None,
            })
            .expect("said")
    };
    // A caption shows a token it reads differently ("service dot go line 43") once it is said
    // whole; the chip comes as its first word starts (I1 holds it to that word).
    let (before, after) = (caption("lookup in"), caption("service.go:43"));
    assert!(
        (before..=after).contains(&chips[0].0),
        "the chip comes with the words: {chips:?} between {before} and {after}"
    );
    assert_eq!(
        dropped(&sim),
        0,
        "a citation is not a tag: nothing is dropped"
    );
}

#[test]
fn clicking_a_source_chip_opens_the_code_card_on_that_line() {
    let mut sim = Sim::new(config());
    reading(&mut sim);
    sim.talker.push(ScriptedReply::says(
        "It starts at service.go:52, the cache check.",
    ));
    sim.user_says(0, "Where does it start?");
    sim.run_until(30_000);
    let source = chips(&sim)
        .first()
        .and_then(|(_, _, source)| source.clone())
        .expect("a chip with its location");
    sim.at(
        31_000,
        Input::Command(UiCommand::OpenSource {
            path: source.path.clone(),
            line: source.line,
        }),
    );
    sim.at(
        32_000,
        Input::Command(UiCommand::OpenSource {
            path: "internal/user/secret.go".into(),
            line: 52,
        }),
    );
    sim.at(
        33_000,
        Input::Command(UiCommand::OpenSource {
            path: PATH.into(),
            line: 61,
        }),
    );
    sim.run_until(40_000);
    assert_clean(&sim);
    let cards: Vec<(u32, usize)> = sim
        .ui()
        .filter_map(|(_, event)| match event {
            UiEvent::SurfaceShow {
                surface:
                    Surface::Code {
                        path,
                        first_line,
                        lines,
                        ..
                    },
            } if path == PATH => Some((*first_line, lines.len())),
            _ => None,
        })
        .collect();
    assert_eq!(
        cards,
        [(48, 13)],
        "four read lines above, the read lines after: only read lines, and only once"
    );
    assert!(sim.ui().any(|(_, event)| matches!(
        event,
        UiEvent::SurfaceUpdate {
            update: SurfaceUpdate::Highlight { line: 52 },
            ..
        }
    )));
}
