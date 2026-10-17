//! A seeded fuzzer over conversation interleavings (LLD §7). Random talker replies (good, bad
//! and malformed narration, tools, refusals, failures) meet random user behaviour (questions,
//! backchannels, interruptions, coughs, pointing, mute, push-to-talk, the Esc stop, yes and no)
//! and a voice that sometimes fails to synthesize a clause, on the simulator's fake clock. Every
//! step is checked for I1–I7, I9 and I11 and the voice's gain; any panic fails the test.
#![allow(clippy::expect_used, reason = "a broken scenario should fail loudly")]

mod common;

use std::collections::BTreeMap;

use boardai_core::conversation::ConversationState;
use boardai_core::effect::{Effect, LogRecord, Milestone};
use boardai_core::engine::text::{CHANGE_QUESTION_END, SYNTHESIS_FAILED_DETAIL};
use boardai_core::harness::HarnessKind;
use boardai_core::ids::{ClauseId, ElementId, LayerId};
use boardai_core::input::{Input, StopReason, TalkerFailure, TalkerFailureKind};
use boardai_core::limits::VOICE_ONSET_DUCK_HOLD_MS;
use boardai_core::protocol::{DrawOp, MicAction, PointerTarget, Speaker, UiCommand, UiEvent};
use boardai_core::sim::talker::ScriptedReply;
use boardai_core::sim::{Sim, USER_FINAL_DELAY_MS, USER_MS_PER_WORD, USER_ONSET_LEAD_MS};
use boardai_core::{CoreConfig, MicMode};
use common::config;

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

const FRAGMENTS: &[&str] = &[
    r#"<plan steps="Client|Edge|LB|DB"/>Here is the plan. "#,
    r#"<step n="2"/>Next, the edge. "#,
    r#"It starts in the <node id="client" label="Browser" kind="actor"/>browser, "#,
    r#"then the <node id="cdn" label="CDN" near="client"/><edge from="client" to="cdn"/>CDN. "#,
    r#"The <node id="lb" label="Load balancer" near="cdn"/><edge from="cdn" to="lb" label="https"/>balancer spreads load. "#,
    r#"It reaches <node id="api" label="API pod" near="lb" stack="3"/>three pods. "#,
    r#"<detour title="cache-aside"/>That's cache-aside. <node id="redis" label="Redis" kind="db"/>Redis sits in front. "#,
    r#"<resume/>So, back to the request. "#,
    r#"<annotate target="lb" mark="circle" text="hot path"/>This is the hot path. "#,
    r#"<strike target="cdn" text="Edge cache"/>Well, call it an edge cache. "#,
    r#"<focus target="api"/>Look at the pods. "#,
    r#"<trace path="client>cdn>lb"/>A request travels this way. "#,
    r#"<code path="internal/user/service.go" lines="40-45"/>Here is the code. <line n="42"/>This line. "#,
    r#"<compare options="Redis|Memcached" criteria="Speed|Persistence"/>Two options. <cell row="0" col="1" mark="yes"/>Redis persists. "#,
    r#"<close surface="all"/>Let's put those away. "#,
    r#"<node id="vpc" label="VPC" kind="group"/><node id="db" label="Postgres" kind="db" in="vpc"/>The database lives in the VPC. "#,
    r#"Broken <node id=bad/> tags and <blink/> are dropped, speech goes on. "#,
    "A long clause that keeps going without a full stop, with commas, with more words, with even more words, until it gets long enough to be cut on a comma somewhere, and then some more. ",
    "Calls getUserByID in service.go at L42. ",
    r#"A <node id="jobs" label="Jobs" kind="queue" near="api" fill="blush"/>queue takes the writes. "#,
    r#"<compare title="Raft vs Paxos" options="Raft|Paxos" criteria="Easy|Used by"/>Two ways. <cell option="Raft" criterion="Easy" mark="yes"/>Raft is easy. "#,
    r#"<annotate line="42" code="40"/>Look at line 42. "#,
    r#"<recap bullets="Cache first|Postgres on a miss"/>To recap, the cache comes first. "#,
    "The client->CDN hop is **cached**.\n- Then the API. ",
    r#"The <participant id="c" label="Client"/>client <participant id="s" label="Server"/><message from="c" to="s" label="SYN"/>calls the server. "#,
    r#"The server <message from="s" to="c" label="ACK" style="return"/>answers and <message from="s" to="s" label="log"/>logs it. "#,
    r#"An array <cells id="arr" count="4" label="nums"/>has four slots, <write target="arr.1" text="7"/>seven in one. "#,
    r#"Slot one <annotate target="arr.1" mark="highlight" fill="mint"/>is next, <strike target="arr.1" text="8"/>now eight. "#,
    r#"The root <node id="n5" label="5" kind="circle"/>five, <node id="n3" label="3" parent="n5" side="left"/>three on its left. "#,
    r#"Buckets <cells id="ht" count="3" vertical="true"/>stand in a column, <node id="k1" label="k1" near="ht.1"/><edge from="ht.1" to="k1"/>one key in bucket one. "#,
    "See internal/user/service.go:42 for the lookup. ",
    // A local model writing tool calls as tags (D-143).
    r#"Noted. <remember kind="note" text="the cache is warm" about="you"/>Here are your notes. <show_memory/>"#,
];

const QUESTIONS: &[&str] = &[
    "walk me through the request path",
    "why is the cache first",
    "show me the code for that",
    "and then what happens",
    "so the leader and",
    "remember that we use Kafka",
    "show me my sessions",
];

/// Memory controls and view closing, which the core answers itself.
const CONTROLS: &[&str] = &[
    "forget that",
    "don't remember this session",
    "yes, keep it",
    "keep it",
    "close that",
    "back to the main board",
    "ok, go on",
    "we can stop here",
];

const CUES: &[&str] = &[
    "wait, why Redis first?",
    "hold on",
    "stop",
    "no, we use Kafka",
    "what's an inode",
];

fn reply(rng: &mut Rng) -> ScriptedReply {
    let mut text = String::new();
    for _ in 0..1 + rng.below(5) {
        text.push_str(rng.pick(FRAGMENTS));
    }
    let mut reply = ScriptedReply::says(&text);
    match rng.below(12) {
        0 => {
            reply = reply.calls(
                "read_file",
                r#"{"path":"internal/user/service.go","start_line":40,"end_line":50}"#,
            );
        }
        1 => {
            reply = reply.calls(
                "research",
                r#"{"question":"where is the TTL","depth":"quick"}"#,
            )
        }
        2 | 8 => reply = reply.calls("propose_change", r#"{"instruction":"add jitter"}"#),
        3 => {
            reply = reply.calls(
                "remember",
                r#"{"kind":"stated","text":"we use Kafka","about":"systems"}"#,
            );
        }
        4 => reply = reply.calls("read_file", r#"{"path":""}"#),
        5 => reply = reply.stops(StopReason::Refusal),
        6 => {
            let failure = TalkerFailure {
                kind: TalkerFailureKind::Timeout,
                detail: "deadline".into(),
            };
            // A call that came before the failure: it is committed, so it needs its result.
            reply = reply
                .calls("list_dir", r#"{"path":"internal"}"#)
                .stops(StopReason::Failed { failure });
        }
        // The decoders deliver a max_tokens turn's calls as invalid (cut-off input).
        7 => {
            reply = reply
                .calls("read_file", r#"{"path":"internal/us"#)
                .stops(StopReason::MaxTokens);
        }
        _ => {}
    }
    reply
}

/// How long the simulated user is busy saying `text`: one person never says two things at once.
fn saying(sim: &mut Sim, at: u64, text: &str) -> u64 {
    sim.user_says(at, text);
    let words = u64::try_from(text.split_whitespace().count()).unwrap_or(1);
    words * USER_MS_PER_WORD + USER_FINAL_DELAY_MS + 50
}

/// [`saying`] as the real voice detector reports it: an onset first, `VoiceStart` after it.
fn saying_with_onset(sim: &mut Sim, at: u64, text: &str) -> u64 {
    sim.user_onset(at);
    USER_ONSET_LEAD_MS + saying(sim, at + USER_ONSET_LEAD_MS, text)
}

/// How the user answers BoardAI's question about a change.
const ANSWERS: &[&str] = &["yes", "yes, go ahead", "go", "no", "hmm, not now"];

/// The longest a user listens for BoardAI to finish asking about a change.
const QUESTION_LISTEN_MS: u64 = 15_000;

/// After the last action, the longest the user waits for BoardAI to finish its reply.
const FINAL_REPLY_WAIT_MS: u64 = 60_000;

/// BoardAI is still talking and its question about a change is queued: asked since the last
/// request, not yet heard whole, not stopped.
fn is_asking_about_a_change(sim: &Sim) -> bool {
    let mut asking = false;
    for (_, effect) in sim.effects() {
        match effect {
            Effect::Synthesize { text, .. } => asking |= text.ends_with(CHANGE_QUESTION_END),
            Effect::Ui(UiEvent::Caption {
                who: Speaker::Assistant,
                text,
                is_final: true,
            }) if text.ends_with(CHANGE_QUESTION_END) => asking = false,
            Effect::Talk { .. } | Effect::StopVoice => asking = false,
            Effect::Ui(_)
            | Effect::Duck { .. }
            | Effect::CancelTalk
            | Effect::RunTool { .. }
            | Effect::RunHarness { .. }
            | Effect::CancelHarness { .. }
            | Effect::Log(_)
            | Effect::Remember(_)
            | Effect::WakeAt { .. }
            | Effect::Forget { .. }
            | Effect::AcceptProposal { .. } => {}
        }
    }
    asking
        && matches!(
            sim.core().state(),
            ConversationState::Speaking | ConversationState::Thinking
        )
}

/// BoardAI has asked about a change and waits: its last words were the question.
fn asked_about_a_change(sim: &Sim) -> bool {
    let last_said = sim
        .ui()
        .filter_map(|(_, event)| {
            let UiEvent::Caption {
                who: Speaker::Assistant,
                text,
                is_final: true,
            } = event
            else {
                return None;
            };
            Some(text)
        })
        .last();
    *sim.core().state() == ConversationState::Idle
        && last_said.is_some_and(|text| text.ends_with(CHANGE_QUESTION_END))
}

/// One random user action at the current time; returns how long to leave the floor alone.
fn act(sim: &mut Sim, rng: &mut Rng) -> u64 {
    let now = sim.now().as_ms();
    let speaking = *sim.core().state() == ConversationState::Speaking;
    match rng.below(12) {
        0 | 1 => saying(sim, now, rng.pick(QUESTIONS)),
        // Right after pointing too: a hover never turns a backchannel into an interruption (I7).
        2 if speaking => {
            sim.user_backchannel(now, rng.pick(&["mm-hm", "yeah", "okay", "right", "got it"]));
            2_000
        }
        3 => saying(sim, now, rng.pick(CUES)),
        4 => {
            let id = rng.pick(&["client", "cdn", "lb", "nothing"]);
            let target = ElementId::parse(id).map(|id| PointerTarget::Element {
                layer: LayerId::MAIN,
                id,
            });
            sim.at(now, Input::Command(UiCommand::Pointer { target }));
            0
        }
        5 => {
            sim.mic(now, MicAction::Mute);
            sim.mic(now + 500 + rng.below(3_000), MicAction::Unmute);
            4_000
        }
        6 => {
            sim.mic(now, MicAction::TalkDown);
            let busy = saying(sim, now + 50, rng.pick(QUESTIONS));
            sim.mic(now + 50 + busy, MicAction::TalkUp);
            busy + 500
        }
        7 | 8 => saying(
            sim,
            now,
            rng.pick(&["yes", "yes, go ahead", "go", "no", "hmm, not now"]),
        ),
        9 => {
            sim.at(now, Input::Command(UiCommand::Stop));
            0
        }
        // A cough, a click: an onset that never becomes speech.
        10 => {
            sim.user_onset(now);
            VOICE_ONSET_DUCK_HOLD_MS + 100
        }
        11 => saying_with_onset(sim, now, rng.pick(CUES)),
        _ => 0,
    }
}

/// How often the interesting paths ran, so the fuzzer can't pass by exercising nothing.
#[derive(Default)]
struct Coverage {
    counts: BTreeMap<&'static str, usize>,
}

/// Whether an effect shows a path was reached.
type Reached = fn(&Effect) -> bool;

/// The paths the fuzzer must reach, as predicates over effects.
const PATHS: &[(&str, Reached)] = &[
    ("stop voice", |effect| matches!(effect, Effect::StopVoice)),
    (
        "change asked",
        |effect| matches!(effect, Effect::Synthesize { text, .. } if text.ends_with(CHANGE_QUESTION_END)),
    ),
    ("onset duck", |effect| {
        *effect
            == Effect::Log(LogRecord::Milestone {
                milestone: Milestone::OnsetDuck,
            })
    }),
    ("backchannel", |effect| {
        matches!(effect, Effect::Ui(UiEvent::Acknowledge))
    }),
    ("freeze", |effect| {
        matches!(effect, Effect::Ui(UiEvent::Freeze { .. }))
    }),
    ("resume", |effect| {
        matches!(effect, Effect::Ui(UiEvent::Resume { .. }))
    }),
    ("detour", |effect| {
        matches!(effect, Effect::Ui(UiEvent::LayerPush { .. }))
    }),
    ("problem", |effect| {
        matches!(effect, Effect::Ui(UiEvent::Problem { .. }))
    }),
    ("op", |effect| {
        matches!(effect, Effect::Ui(UiEvent::OpStart { .. }))
    }),
    ("camera", |effect| {
        matches!(effect, Effect::Ui(UiEvent::Camera { .. }))
    }),
    (
        "change",
        |effect| matches!(effect, Effect::RunHarness { spec, .. } if matches!(spec.kind, HarnessKind::Change { .. })),
    ),
    ("dropped", |effect| {
        matches!(effect, Effect::Log(LogRecord::Dropped { .. }))
    }),
    (
        "skipped clause",
        |effect| matches!(effect, Effect::Ui(UiEvent::Problem { detail, .. }) if detail == SYNTHESIS_FAILED_DETAIL),
    ),
    ("forget", |effect| matches!(effect, Effect::Forget { .. })),
    ("keep", |effect| {
        matches!(effect, Effect::AcceptProposal { .. })
    }),
    ("view", |effect| {
        matches!(effect, Effect::Ui(UiEvent::ViewShow { .. }))
    }),
    ("view closed", |effect| {
        matches!(effect, Effect::Ui(UiEvent::ViewDismiss))
    }),
    ("detour closed", |effect| {
        matches!(effect, Effect::Ui(UiEvent::LayerPop { .. }))
    }),
    ("session end", |effect| {
        matches!(effect, Effect::Ui(UiEvent::SessionEnd))
    }),
    ("diagram", |effect| {
        matches!(
            effect,
            Effect::Ui(UiEvent::OpStart {
                op: DrawOp::Participant { .. }
                    | DrawOp::Message { .. }
                    | DrawOp::Cells { .. }
                    | DrawOp::Write { .. },
                ..
            })
        )
    }),
    ("lifeline growth", |effect| {
        matches!(
            effect,
            Effect::Ui(UiEvent::OpStart { op: DrawOp::Message { extend, .. }, .. }) if !extend.is_empty()
        )
    }),
];

/// Clauses whose synthesis fails, about one in [`FAILED_CLAUSE_ONE_IN`], from their own generator
/// so the user and talker behaviour of each seed stays what it was.
const FAILED_CLAUSE_ONE_IN: u64 = 25;
const CLAUSES_PER_SEED_MAX: u32 = 400;

fn fail_some_clauses(sim: &mut Sim, seed: u64) {
    let mut rng = Rng(seed.wrapping_mul(0xd1b5_4a32_d192_ed03) | 1);
    for clause in 0..CLAUSES_PER_SEED_MAX {
        if rng.below(FAILED_CLAUSE_ONE_IN) == 0 {
            sim.speech.fail(ClauseId(clause));
        }
    }
}

impl Coverage {
    fn add(&mut self, sim: &Sim) {
        for (_, effect) in sim.effects() {
            for (name, reached) in PATHS {
                if reached(effect) {
                    *self.counts.entry(name).or_default() += 1;
                }
            }
        }
    }

    fn assert_exercised(&self, names: &[&str]) {
        for name in names {
            assert!(
                self.counts.get(name).copied().unwrap_or(0) > 0,
                "{name} never ran: {:?}",
                self.counts
            );
        }
    }
}

/// Memory tool calls added to some replies, from their own generator so the rest of each seed's
/// talker and user behaviour stays what it was.
fn add_memory_calls(reply: ScriptedReply, memory: &mut Rng) -> ScriptedReply {
    match memory.below(16) {
        0 => reply.calls(
            "remember",
            r#"{"kind":"inferred","text":"likes a slower pace","about":"you"}"#,
        ),
        1 => reply.calls("show_library", "{}"),
        2 => reply.calls("add_task", r#"{"title":"check the TTLs"}"#),
        _ => reply,
    }
}

fn run_seed(seed: u64, mic: MicMode, coverage: &mut Coverage) {
    let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
    let mut memory = Rng(seed.wrapping_mul(0xbf58_476d_1ce4_e5b9) | 1);
    let mut sim = Sim::new(CoreConfig {
        mic,
        suggestion: Some("Pick up where we left off?".to_owned()),
        ..config()
    });
    sim.talker.chunk_interval_ms = 10 + rng.below(300);
    sim.talker.first_token_ms = 100 + rng.below(1_500);
    sim.harness.run_ms = 1_000 + rng.below(10_000);
    for _ in 0..40 {
        let reply = add_memory_calls(reply(&mut rng), &mut memory);
        sim.talker.push(reply);
    }
    fail_some_clauses(&mut sim, seed);
    sim.user_says(0, "walk me through the request path");
    for _ in 0..30 {
        // A user who listens to BoardAI's question about a change before doing anything else.
        if is_asking_about_a_change(&sim) && memory.below(3) > 0 {
            let until = sim.now().as_ms() + QUESTION_LISTEN_MS;
            sim.run_until_true(until, |sim| *sim.core().state() == ConversationState::Idle);
            continue;
        }
        let quiet = if asked_about_a_change(&sim) && memory.below(3) > 0 {
            let now = sim.now().as_ms();
            saying(&mut sim, now, memory.pick(ANSWERS))
        } else if memory.below(6) == 0 {
            let now = sim.now().as_ms();
            saying(&mut sim, now, memory.pick(CONTROLS))
        } else {
            act(&mut sim, &mut rng)
        };
        let until = sim.now().as_ms() + quiet.max(100 + rng.below(4_000));
        sim.run_until(until);
    }
    // BoardAI finishes what it is saying, and a question about a change at its end gets its
    // answer too.
    let until = sim.now().as_ms() + FINAL_REPLY_WAIT_MS;
    sim.run_until_true(until, |sim| *sim.core().state() == ConversationState::Idle);
    if asked_about_a_change(&sim) {
        let now = sim.now().as_ms();
        saying(&mut sim, now, memory.pick(ANSWERS));
    }
    let end = sim.now().as_ms() + 120_000;
    sim.run_until(end);
    assert!(
        sim.violations().is_empty(),
        "seed {seed}: {:#?}",
        sim.violations()
    );
    let settled = matches!(
        sim.core().state(),
        ConversationState::Idle | ConversationState::Muted | ConversationState::Listening(_)
    );
    assert!(
        settled,
        "seed {seed} ended in {}",
        sim.core().state().name()
    );
    coverage.add(&sim);
}

#[test]
fn random_interleavings_keep_every_invariant_open_mic() {
    let mut coverage = Coverage::default();
    for seed in 1..=150 {
        run_seed(seed, MicMode::OpenMic, &mut coverage);
    }
    coverage.assert_exercised(&[
        "diagram",
        "lifeline growth",
        "stop voice",
        "onset duck",
        "backchannel",
        "freeze",
        "resume",
        "detour",
        "problem",
        "op",
        "camera",
        "change asked",
        "change",
        "dropped",
        "skipped clause",
        "forget",
        "keep",
        "view",
        "view closed",
        "detour closed",
        "session end",
    ]);
}

#[test]
fn random_interleavings_keep_every_invariant_push_to_talk() {
    let mut coverage = Coverage::default();
    for seed in 1_000..1_050 {
        run_seed(seed, MicMode::PushToTalk, &mut coverage);
    }
    // Voice without the key is ignored, so there are no backchannels to exercise.
    coverage.assert_exercised(&[
        "stop voice",
        "freeze",
        "resume",
        "detour",
        "problem",
        "op",
        "dropped",
        "skipped clause",
    ]);
}
