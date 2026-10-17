//! Simulator scenarios for the drawing path (LLD §2.4, §2.5, §3.7): the orb while drawing, the
//! camera following the board, the tones the core picks, and the surfaces a reply opens.
#![allow(clippy::expect_used, reason = "a broken scenario should fail loudly")]
#![allow(
    clippy::wildcard_enum_match_arm,
    reason = "scenarios pick one event kind out of the whole trace"
)]

mod common;

use std::collections::BTreeSet;

use boardai_core::geometry::Rect;
use boardai_core::ids::LayerId;
use boardai_core::protocol::{
    ColumnRange, DrawOp, Fill, Mark, OrbState, Surface, SurfaceUpdate, UiEvent,
};
use boardai_core::sim::Sim;
use boardai_core::sim::talker::ScriptedReply;
use boardai_core::tools::ToolOutput;
use common::{DEMO, assert_clean, config, dropped, op_starts, orbs};

/// Regression: the orb's drawing linger started at session time zero, so a reply that spoke
/// within 800 ms of the session start showed the orb drawing with nothing drawn.
#[test]
fn the_orb_shows_speaking_not_drawing_before_anything_is_drawn() {
    let mut sim = Sim::new(config());
    sim.talker.first_token_ms = 10;
    sim.talker.push(ScriptedReply::says("Hello there."));
    sim.user_says_for(0, "Hi.", 50);
    sim.run_until(5_000);
    assert_clean(&sim);
    let speaking_at = sim
        .ui()
        .find(|(_, event)| {
            matches!(
                event,
                UiEvent::Orb {
                    state: OrbState::Speaking | OrbState::Drawing,
                    ..
                }
            )
        })
        .map(|(at, _)| at.as_ms())
        .expect("the reply was spoken");
    assert!(
        speaking_at < 800,
        "the reply must start inside the linger window"
    );
    let orbs = orbs(&sim);
    assert!(orbs.contains(&OrbState::Speaking), "{orbs:?}");
    assert!(!orbs.contains(&OrbState::Drawing), "{orbs:?}");
}

fn frames(sim: &Sim, layer: LayerId) -> Vec<(u64, Rect, Option<String>)> {
    sim.ui()
        .filter_map(|(at, event)| match event {
            UiEvent::Camera {
                layer: framed,
                frame,
                focus,
                ..
            } if *framed == layer => Some((
                at.as_ms(),
                *frame,
                focus.as_ref().map(|id| id.as_str().to_owned()),
            )),
            _ => None,
        })
        .collect()
}

/// The camera frames the board with its first stroke and follows as it grows: every stroke
/// starts inside the frame (I11, checked on every op by the simulator), frames only grow, and a
/// frame is sent with the op that needed it, before it.
#[test]
fn the_camera_frames_the_first_stroke_and_follows_the_board() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(DEMO));
    sim.user_says(0, "Walk me through how a request reaches our API.");
    sim.run_until(30_000);
    assert_clean(&sim);
    let frames = frames(&sim, LayerId::MAIN);
    assert!(frames.len() >= 3, "framed, then followed: {frames:?}");
    let first_op = op_starts(&sim)[0].0.as_ms();
    assert_eq!(
        frames[0].0, first_op,
        "the first frame comes with the first stroke"
    );
    let events: Vec<&UiEvent> = sim.ui().map(|(_, event)| event).collect();
    let camera = events
        .iter()
        .position(|event| matches!(event, UiEvent::Camera { .. }))
        .expect("a camera frame");
    let op = events
        .iter()
        .position(|event| matches!(event, UiEvent::OpStart { .. }))
        .expect("an op");
    assert!(
        camera < op,
        "the frame goes out before the stroke it is for"
    );
    for pair in frames.windows(2) {
        assert!(pair[1].1.contains(pair[0].1), "frames only grow: {pair:?}");
    }
    let board = sim
        .core()
        .board()
        .layer(LayerId::MAIN)
        .and_then(boardai_core::board::Layer::bounds)
        .expect("drawn");
    let last = frames.last().expect("a frame").1;
    assert!(last.contains(board), "the last frame holds the whole board");
    assert!(frames.iter().all(|(_, _, focus)| focus.is_none()));
}

/// A `focus` tag frames one element and holds while strokes land inside it; a stroke outside it
/// sends the camera back to following the whole board.
#[test]
fn a_focus_holds_until_the_drawing_moves_on() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(concat!(
        r#"The <node id="lb" label="Load balancer"/>load balancer is the hot spot. "#,
        r#"<focus target="lb"/>Look at it closely: <annotate target="lb" mark="circle"/>this "#,
        r#"is where the traffic turns. Behind it sit <node id="api" label="API pod" near="lb"/>"#,
        r#"<node id="api2" label="Worker" near="api"/><node id="api3" label="Cron" near="api2"/>"#,
        r#"the workers."#,
    )));
    sim.user_says(0, "Where is the bottleneck?");
    sim.run_until(30_000);
    assert_clean(&sim);
    let frames = frames(&sim, LayerId::MAIN);
    let focus = frames
        .iter()
        .position(|(_, _, focus)| focus.as_deref() == Some("lb"))
        .expect("focused");
    let annotate_at = op_starts(&sim)
        .iter()
        .find(|(_, event)| {
            matches!(
                event,
                UiEvent::OpStart {
                    op: DrawOp::Annotate { .. },
                    ..
                }
            )
        })
        .map(|(at, _)| at.as_ms())
        .expect("the circle was drawn");
    assert!(
        frames[focus + 1..]
            .iter()
            .all(|(at, _, _)| *at > annotate_at),
        "the circle inside the focus moved nothing: {frames:?}"
    );
    let after = frames.get(focus + 1).expect("the camera follows again");
    assert!(after.2.is_none(), "following the board clears the focus");
}

/// A strike outside a held focus (or a group growing past it) must still be seen: the camera
/// follows before the stroke starts. Found by the fuzzer (seed 129) through I11.
#[test]
fn a_stroke_outside_a_held_focus_moves_the_camera() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(concat!(
        r#"The <node id="cdn" label="CDN"/>CDN sits in front of the "#,
        r#"<node id="lb" label="Load balancer" near="cdn"/>balancer. "#,
        r#"<focus target="cdn"/>Look at the CDN. Actually, the second box is "#,
        r#"<strike target="lb" text="Gateway"/>a gateway."#,
    )));
    sim.user_says(0, "What is in front?");
    sim.run_until(30_000);
    assert_clean(&sim);
    let frames = frames(&sim, LayerId::MAIN);
    let focus = frames
        .iter()
        .position(|(_, _, focus)| focus.as_deref() == Some("cdn"))
        .expect("focused");
    let strike_at = op_starts(&sim)
        .iter()
        .find(|(_, event)| {
            matches!(
                event,
                UiEvent::OpStart {
                    op: DrawOp::Strike { .. },
                    ..
                }
            )
        })
        .map(|(at, _)| at.as_ms())
        .expect("the strike was drawn");
    let moved = frames[focus + 1..]
        .iter()
        .any(|(at, _, focus)| *at == strike_at && focus.is_none());
    assert!(
        moved,
        "the camera left the focus for the strike: {frames:?}"
    );
}

/// The core picks each node's tone: the demo's CDN sky, load balancer lilac, API pods mint,
/// and a talker's own `fill` wins.
#[test]
fn the_core_picks_tones_and_the_talker_may_choose() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(concat!(
        r#"It starts in the <node id="client" label="Browser" kind="actor"/>browser, hits the "#,
        r#"<node id="cdn" label="CDN" near="client"/>CDN, then the "#,
        r#"<node id="lb" label="Load balancer" near="cdn"/>load balancer, the "#,
        r#"<node id="api" label="API pod" near="lb" stack="3"/>pods, "#,
        r#"<node id="redis" label="Redis" kind="db" near="api"/>Redis and a "#,
        r#"<node id="jobs" label="Jobs" kind="queue" near="redis" fill="blush"/>queue."#,
    )));
    sim.user_says(0, "Walk me through it.");
    sim.run_until(30_000);
    assert_clean(&sim);
    let fills: Vec<(String, Option<Fill>)> = op_starts(&sim)
        .iter()
        .filter_map(|(_, event)| match event {
            UiEvent::OpStart {
                op: DrawOp::Node { id, fill, .. },
                ..
            } => Some((id.as_str().to_owned(), *fill)),
            _ => None,
        })
        .collect();
    let expected: Vec<(String, Option<Fill>)> = [
        ("client", None),
        ("cdn", Some(Fill::Sky)),
        ("lb", Some(Fill::Lilac)),
        ("api", Some(Fill::Mint)),
        ("redis", Some(Fill::Sky)),
        ("jobs", Some(Fill::Blush)),
    ]
    .into_iter()
    .map(|(id, fill)| (id.to_owned(), fill))
    .collect();
    assert_eq!(fills, expected);
}

/// A detour opened in answer to an interruption carries the question without its lead-in; a
/// detour the talker opens on an empty board, answering a first question, carries none.
#[test]
fn a_detour_card_carries_the_question_that_opened_it() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(DEMO));
    sim.talker.push(ScriptedReply::says(concat!(
        r#"<detour title="Cache-aside"/>Memory is fast, so the <node id="api" label="API"/>API "#,
        r#"asks <node id="redis" label="Redis" kind="db" near="api"/>Redis first. "#,
        r#"<resume/>So, back to the load balancer."#,
    )));
    sim.user_says(0, "Walk me through how a request reaches our API.");
    assert!(sim.run_until_true(30_000, |sim| op_starts(sim).len() > 2));
    let at = sim.now().as_ms() + 50;
    sim.user_says_for(at, "Wait — why Redis first?", 900);
    sim.run_until(at + 30_000);
    assert_clean(&sim);
    let pushes: Vec<Option<String>> = sim
        .ui()
        .filter_map(|(_, event)| match event {
            UiEvent::LayerPush { question, .. } => Some(question.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(pushes, vec![Some("why Redis first?".to_owned())]);
    let detour = frames(&sim, LayerId(1));
    assert!(!detour.is_empty(), "the detour layer has its own camera");

    let mut fresh = Sim::new(config());
    fresh.talker.push(ScriptedReply::says(
        r#"<detour title="Aside"/>A quick aside first."#,
    ));
    fresh.user_says(0, "Explain consensus.");
    fresh.run_until(20_000);
    let question = fresh.ui().find_map(|(_, event)| match event {
        UiEvent::LayerPush { question, .. } => Some(question.clone()),
        _ => None,
    });
    assert_eq!(question, Some(None));
}

/// Frame 5: the pen underlines `s.cache.Get(` on line 43 as the voice reaches "asks"; the core
/// finds the columns on the line it read.
#[test]
fn a_code_line_mark_underlines_the_text_the_talker_names() {
    let mut sim = Sim::new(config());
    let lines: Vec<String> = [
        "func (s *UserService) GetUser(ctx context.Context, id string) (*User, error) {",
        "    if u, err := s.cache.Get(ctx, userKey(id)); err == nil {",
        "        return u, nil // cache hit",
    ]
    .map(str::to_owned)
    .to_vec();
    sim.tools.respond(
        "read_file",
        ToolOutput::File {
            path: "internal/user/service.go".into(),
            first_line: 42,
            lines,
            more_lines: 0,
        },
    );
    sim.talker
        .push(ScriptedReply::says("Let me open it.").calls(
            "read_file",
            r#"{"path":"internal/user/service.go","start_line":42,"end_line":44}"#,
        ));
    sim.talker.push(ScriptedReply::says(concat!(
        r#"<code path="internal/user/service.go" lines="42-44"/>In the code, <line n="43"/>"#,
        r#"line 43 <annotate line="43" code="s.cache.Get("/>asks Redis first, and "#,
        r#"<annotate line="44" code="redis.Get"/>that is it."#,
    )));
    sim.user_says(0, "show me the lookup");
    sim.run_until(30_000);
    assert_clean(&sim);
    let marks: Vec<(u32, ColumnRange, Mark)> = sim
        .ui()
        .filter_map(|(_, event)| match event {
            UiEvent::SurfaceUpdate {
                update:
                    SurfaceUpdate::Mark {
                        line,
                        columns,
                        mark,
                    },
                ..
            } => Some((*line, *columns, *mark)),
            _ => None,
        })
        .collect();
    assert_eq!(
        marks,
        vec![(43, ColumnRange { begin: 17, end: 29 }, Mark::Underline)],
        "text not on the line is dropped"
    );
    assert!(dropped(&sim) >= 1);
}

/// Compare cards: options are columns and criteria rows, with the talker's title; a recap tag
/// opens the recap card.
#[test]
fn compare_and_recap_cards_open_from_narration() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(concat!(
        r#"<compare title="Raft vs Paxos" options="Raft|Paxos" criteria="Easy to follow|Used by"/>"#,
        r#"Both reach consensus, but <cell option="Raft" criterion="Easy to follow" mark="yes"/>"#,
        r#"Raft is easier, and <cell criterion="Used by" option="Raft" text="etcd"/>etcd uses it. "#,
        r#"<recap bullets="Raft is easier|etcd uses Raft|Paxos came first"/>That's the gist."#,
    )));
    sim.user_says(0, "Compare Raft and Paxos.");
    sim.run_until(30_000);
    assert_clean(&sim);
    let title = sim.ui().find_map(|(_, event)| match event {
        UiEvent::SurfaceShow {
            surface: Surface::Compare { title, .. },
        } => Some(title.clone()),
        _ => None,
    });
    assert_eq!(title, Some(Some("Raft vs Paxos".to_owned())));
    let cells: Vec<(u8, u8)> = sim
        .ui()
        .filter_map(|(_, event)| match event {
            UiEvent::SurfaceUpdate {
                update: SurfaceUpdate::Cell { row, col, .. },
                ..
            } => Some((*row, *col)),
            _ => None,
        })
        .collect();
    assert_eq!(
        cells,
        vec![(0, 0), (1, 0)],
        "rows are criteria, columns options"
    );
    let recap = sim.ui().find_map(|(_, event)| match event {
        UiEvent::SurfaceShow {
            surface: Surface::Recap { bullets, .. },
        } => Some(bullets.len()),
        _ => None,
    });
    assert_eq!(recap, Some(3));
}

/// The whole session exports as one `.excalidraw` document: a frame per layer that has drawing,
/// every element inside its frame, ids unique, bindings real.
#[test]
fn the_session_exports_with_a_frame_per_layer() {
    let mut sim = Sim::new(config());
    sim.talker.push(ScriptedReply::says(DEMO));
    sim.talker.push(ScriptedReply::says(concat!(
        r#"<detour title="Cache-aside"/>Memory is fast, so the <node id="api" label="API"/>API "#,
        r#"asks <node id="redis" label="Redis" kind="db" near="api"/>Redis first. "#,
        r#"<resume/>So, back to the load balancer."#,
    )));
    sim.user_says(0, "Walk me through how a request reaches our API.");
    assert!(sim.run_until_true(30_000, |sim| op_starts(sim).len() > 2));
    let at = sim.now().as_ms() + 50;
    sim.user_says_for(at, "Wait — why Redis first?", 900);
    sim.run_until(at + 30_000);
    assert_clean(&sim);
    let text = sim.core().export_session_excalidraw();
    assert_eq!(
        text,
        sim.core().export_session_excalidraw(),
        "deterministic"
    );
    let document: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
    assert_eq!(document["type"], "excalidraw");
    assert_eq!(document["version"], 2);
    let elements = document["elements"].as_array().expect("elements");
    let frames: Vec<&serde_json::Value> = elements
        .iter()
        .filter(|element| element["type"] == "frame")
        .collect();
    let names: Vec<&str> = frames
        .iter()
        .filter_map(|frame| frame["name"].as_str())
        .collect();
    assert_eq!(names, vec!["Main board", "Detour: Cache-aside"]);
    let ids: BTreeSet<&str> = elements
        .iter()
        .filter_map(|element| element["id"].as_str())
        .collect();
    assert_eq!(ids.len(), elements.len(), "unique ids");
    let rect = |element: &serde_json::Value| {
        let number = |key: &str| element[key].as_i64().expect("a number");
        (number("x"), number("y"), number("width"), number("height"))
    };
    for element in elements.iter().filter(|element| element["type"] != "frame") {
        let frame_id = element["frameId"]
            .as_str()
            .expect("every element has a frame");
        let frame = frames
            .iter()
            .find(|frame| frame["id"] == frame_id)
            .expect("its frame exists");
        let (fx, fy, fw, fh) = rect(frame);
        let (x, y, _, _) = rect(element);
        assert!(
            x >= fx && y >= fy && x <= fx + fw && y <= fy + fh,
            "{} sits in {frame_id}",
            element["id"]
        );
        for end in ["containerId", "startBinding", "endBinding"] {
            let target = element[end]
                .as_str()
                .or_else(|| element[end]["elementId"].as_str());
            if let Some(target) = target {
                assert!(ids.contains(target), "{end} points at {target}");
            }
        }
    }
    let (a, b) = (rect(frames[0]), rect(frames[1]));
    assert!(
        a.0 + a.2 < b.0,
        "frames sit side by side without overlapping"
    );
}
