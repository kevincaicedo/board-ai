//! What a session keeps of its sources, and what it lets go. Off the record nothing more is
//! kept (I10): no brief is sent to be cached, and nothing is kept in a project (D-272). A
//! candidate the user didn't pick is let go, but never a source the session already shares: the
//! runtime names a path by the id it already has. Review 3, findings 3 and 12: each test was red
//! at 2b76809.
#![allow(clippy::expect_used, reason = "a broken scenario should fail loudly")]
#![allow(
    clippy::wildcard_enum_match_arm,
    reason = "scenarios pick one event kind out of the whole trace"
)]

mod common;

use boardai_core::effect::Effect;
use boardai_core::input::Input;
use boardai_core::protocol::UiEvent;
use boardai_core::sim::Sim;
use boardai_core::sim::sources::code_folder;
use boardai_core::sources::{SourceEffect, SourceId, SourceInput};
use common::{assert_clean, config};

const OVERVIEW: &str = "# Brief: inf-alloc\n## Overview\nAn arena allocator.\n## Components\n\
                        - arena · Arena · module · src/arena.rs — hands out slots\n";

fn opened(sim: &mut Sim) {
    sim.at(
        0,
        Input::Source {
            source: SourceInput::Opened {
                harness: Some("Claude Code".into()),
                project: None,
            },
        },
    );
}

/// Finding 3 (I10, the core's half): a brief that arrived after "don't remember this session"
/// was still sent to be cached for a kept source, and the runtime saves that in the project's
/// vault.
#[test]
fn a_brief_after_going_off_the_record_is_never_cached() {
    let mut sim = Sim::new(config());
    sim.sources.default_brief = OVERVIEW.to_owned();
    opened(&mut sim);
    // Dropped on the right half: kept in a project; its overview brief starts (3 s here).
    sim.share(
        100,
        &code_folder(1, "inf-alloc", "~/…/infinitydb/crates/inf-alloc"),
        true,
    );
    sim.user_says(600, "Don't remember this session.");
    sim.run_until(20_000);
    let forgot = sim
        .effects()
        .find_map(|(at, effect)| matches!(effect, Effect::Forget { .. }).then_some(at))
        .expect("the session was forgotten");
    let kept_after: Vec<String> = sim
        .effects()
        .filter(|(at, effect)| {
            *at > forgot
                && matches!(
                    effect,
                    Effect::Source {
                        source: SourceEffect::CacheBrief { .. } | SourceEffect::Keep { .. }
                    }
                )
        })
        .map(|(at, effect)| format!("{at:?}: {effect:?}"))
        .collect();
    assert!(
        kept_after.is_empty(),
        "forgot at {forgot:?}; then: {kept_after:#?}"
    );
    assert_clean(&sim);
}

/// Finding 12: "use the alloc folder" found the shared folder and another; picking the other
/// discarded every candidate not picked, the shared one included, and the runtime stopped
/// letting the talker read it.
#[test]
fn picking_another_candidate_keeps_a_shared_source_readable() {
    let mut sim = Sim::new(config());
    let alloc = code_folder(1, "alloc", "~/code/alloc");
    let vendor = code_folder(2, "alloc", "~/code/other/vendor/alloc");
    sim.sources.folders = vec![alloc.clone(), vendor];
    opened(&mut sim);
    sim.share(100, &alloc, false);
    sim.run_until(1_000);
    sim.user_says(2_000, "Use the alloc folder.");
    sim.run_until(6_000);
    sim.user_says(6_000, "The last one.");
    sim.run_until(12_000);
    let discarded: Vec<SourceId> = sim
        .effects()
        .filter_map(|(_, effect)| match effect {
            Effect::Source {
                source: SourceEffect::Discard { sources },
            } => Some(sources.clone()),
            Effect::Source { .. }
            | Effect::Talk { .. }
            | Effect::CancelTalk
            | Effect::Synthesize { .. }
            | Effect::StopVoice
            | Effect::Duck { .. }
            | Effect::RunTool { .. }
            | Effect::RunHarness { .. }
            | Effect::CancelHarness { .. }
            | Effect::Remember(_)
            | Effect::AcceptProposal { .. }
            | Effect::Forget { .. }
            | Effect::Ui(_)
            | Effect::Log(_)
            | Effect::WakeAt { .. } => None,
        })
        .flatten()
        .collect();
    assert!(
        !discarded.contains(&SourceId(1)),
        "the shared folder was discarded: {discarded:?}"
    );
    assert!(
        sim.sources.readable_sources.contains(&SourceId(1)),
        "the shared folder stays readable"
    );
    let tray = sim
        .ui()
        .filter_map(|(_, event)| match event {
            UiEvent::Sources { tray } => Some(tray.clone()),
            _ => None,
        })
        .last()
        .expect("the tray");
    assert!(
        tray.session.iter().any(|row| row.id == SourceId(1)),
        "and shared: {tray:?}"
    );
    assert_clean(&sim);
}
