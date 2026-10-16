//! The conversation simulator (LLD §7): scripted timelines of inputs on a fake clock, with a fake
//! speech-out, a scripted talker, scripted tools and a scripted harness. It steps the real
//! [`Core`], applies its effects the way the runtime would, and checks the invariants after
//! every step.
//!
//! Behind the `sim` feature: the crate's tests enable it, and the runtime's text dev loop can
//! reuse the fake speech. It is never part of the shipped app.

pub mod check;
pub mod speech;
pub mod talker;

use std::collections::BTreeMap;

use crate::effect::Effect;
use crate::engine::{Core, CoreConfig};
use crate::input::Input;
use crate::protocol::{MicAction, UiCommand, UiEvent};
use crate::time::SessionTime;
use check::Checker;
use speech::{FakeSpeech, PLAYBACK_REPORT_MS, STOP_FADE_MS};
use talker::{FakeHarness, FakeTalker, FakeTools};

/// How long a scripted user takes per word, and when speech-to-text sends a partial.
pub const USER_MS_PER_WORD: u64 = 280;
pub const USER_PARTIAL_EVERY_MS: u64 = 500;
pub const USER_FINAL_DELAY_MS: u64 = 120;

/// How long the runtime takes to answer a tool call.
pub const TOOL_LATENCY_MS: u64 = 15;

/// A stamped entry of the trace: everything that went into and came out of the core.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    Input(Input),
    Effect(Effect),
}

#[derive(Clone, Debug, PartialEq)]
enum Event {
    /// A scripted input (user, UI) or a runtime answer.
    Input(Input),
    /// A talker stream input, dropped when its stream was cancelled.
    Talker { generation: u64, input: Input },
    /// A playback report tick.
    Speech,
}

pub struct Sim {
    core: Core,
    now: SessionTime,
    events: BTreeMap<(SessionTime, u64), Event>,
    sequence: u64,
    speech_tick: Option<SessionTime>,
    pub speech: FakeSpeech,
    pub talker: FakeTalker,
    pub tools: FakeTools,
    pub harness: FakeHarness,
    trace: Vec<(SessionTime, Entry)>,
    checker: Checker,
}

impl Sim {
    /// A session started at time zero.
    #[must_use]
    pub fn new(config: CoreConfig) -> Self {
        let mut sim = Self {
            core: Core::new(config),
            now: SessionTime::ZERO,
            events: BTreeMap::new(),
            sequence: 0,
            speech_tick: None,
            speech: FakeSpeech::default(),
            talker: FakeTalker::default(),
            tools: FakeTools::default(),
            harness: FakeHarness::default(),
            trace: Vec::new(),
            checker: Checker::default(),
        };
        let mut effects = Vec::new();
        sim.core.start(SessionTime::ZERO, &mut effects);
        sim.apply(effects);
        sim
    }

    #[must_use]
    pub fn core(&self) -> &Core {
        &self.core
    }

    #[must_use]
    pub fn now(&self) -> SessionTime {
        self.now
    }

    /// Everything that happened, in order.
    #[must_use]
    pub fn trace(&self) -> &[(SessionTime, Entry)] {
        &self.trace
    }

    #[must_use]
    pub fn violations(&self) -> &[String] {
        self.checker.violations()
    }

    /// Every effect, with its time.
    pub fn effects(&self) -> impl Iterator<Item = (SessionTime, &Effect)> {
        self.trace.iter().filter_map(|(at, entry)| match entry {
            Entry::Effect(effect) => Some((*at, effect)),
            Entry::Input(_) => None,
        })
    }

    /// Every UI event, with its time.
    pub fn ui(&self) -> impl Iterator<Item = (SessionTime, &UiEvent)> {
        self.effects().filter_map(|(at, effect)| match effect {
            Effect::Ui(event) => Some((at, event)),
            Effect::Duck { .. }
            | Effect::StopVoice
            | Effect::Synthesize { .. }
            | Effect::Talk { .. }
            | Effect::CancelTalk
            | Effect::RunTool { .. }
            | Effect::RunHarness { .. }
            | Effect::CancelHarness { .. }
            | Effect::Log(_)
            | Effect::Remember(_)
            | Effect::WakeAt { .. }
            | Effect::Forget { .. }
            | Effect::AcceptProposal { .. } => None,
        })
    }

    /// Schedules an input at `at_ms` (never in the past).
    pub fn at(&mut self, at_ms: u64, input: Input) {
        let at = SessionTime::from_ms(at_ms).max(self.now);
        self.schedule(at, Event::Input(input));
    }

    /// The user says `text` starting at `at_ms`, at a natural pace: voice start, partials every
    /// 500 ms, voice end, then the final transcript.
    pub fn user_says(&mut self, at_ms: u64, text: &str) {
        let words = text.split_whitespace().count().max(1);
        let voiced_ms = USER_MS_PER_WORD * u64::try_from(words).unwrap_or(1);
        self.user_says_for(at_ms, text, voiced_ms);
    }

    /// Like [`Sim::user_says`] with an explicit voiced length.
    pub fn user_says_for(&mut self, at_ms: u64, text: &str, voiced_ms: u64) {
        let words: Vec<&str> = text.split_whitespace().collect();
        self.at(at_ms, Input::VoiceStart);
        let mut partial_at = USER_PARTIAL_EVERY_MS;
        while partial_at < voiced_ms {
            let said = usize::try_from(partial_at / USER_MS_PER_WORD)
                .unwrap_or(0)
                .min(words.len());
            if said > 0 {
                let text = words[..said].join(" ");
                self.at(
                    at_ms + partial_at,
                    Input::Transcript {
                        text,
                        is_final: false,
                    },
                );
            }
            partial_at += USER_PARTIAL_EVERY_MS;
        }
        let voiced = u32::try_from(voiced_ms).unwrap_or(u32::MAX);
        self.at(at_ms + voiced_ms, Input::VoiceEnd { voiced_ms: voiced });
        let text = text.to_owned();
        self.at(
            at_ms + voiced_ms + USER_FINAL_DELAY_MS,
            Input::Transcript {
                text,
                is_final: true,
            },
        );
    }

    /// A short backchannel over BoardAI's voice; I7 is checked for it.
    pub fn user_backchannel(&mut self, at_ms: u64, text: &str) {
        self.run_until(at_ms);
        self.checker.expect_backchannel(self.now, &self.core);
        self.user_says(at_ms, text);
    }

    pub fn mic(&mut self, at_ms: u64, action: MicAction) {
        self.at(at_ms, Input::Command(UiCommand::Mic { action }));
    }

    /// Runs every event up to and including `until_ms`.
    pub fn run_until(&mut self, until_ms: u64) {
        let until = SessionTime::from_ms(until_ms);
        while let Some(entry) = self.events.first_entry() {
            if entry.key().0 > until {
                break;
            }
            let ((at, _), event) = entry.remove_entry();
            self.now = at;
            self.handle(event);
        }
        self.now = self.now.max(until);
    }

    /// Runs until `done` holds after a step, or `limit_ms` passes; true when `done` held.
    pub fn run_until_true(&mut self, limit_ms: u64, mut done: impl FnMut(&Self) -> bool) -> bool {
        let limit = SessionTime::from_ms(limit_ms);
        while let Some(entry) = self.events.first_entry() {
            if entry.key().0 > limit {
                break;
            }
            let ((at, _), event) = entry.remove_entry();
            self.now = at;
            self.handle(event);
            if done(self) {
                return true;
            }
        }
        false
    }

    fn schedule(&mut self, at: SessionTime, event: Event) {
        self.sequence += 1;
        self.events.insert((at, self.sequence), event);
    }

    fn handle(&mut self, event: Event) {
        match event {
            Event::Input(input) => {
                if let Input::ClauseReady { clause, .. } = &input {
                    self.speech.mark_ready(*clause);
                    self.arm_speech();
                }
                self.feed(input);
            }
            Event::Talker { generation, input } => {
                if generation == self.talker.generation() {
                    self.feed(input);
                }
            }
            Event::Speech => {
                self.speech_tick = None;
                for input in self.speech.tick(self.now) {
                    self.feed(input);
                }
                self.arm_speech();
            }
        }
    }

    /// Steps the core with one input and applies what it asks for.
    fn feed(&mut self, input: Input) {
        self.checker.on_input(self.now, &input, &self.core);
        self.trace.push((self.now, Entry::Input(input.clone())));
        let mut effects = Vec::new();
        self.core.step(self.now, input, &mut effects);
        self.apply(effects);
        self.checker
            .after_step(self.now, &self.core, &self.talker.scripts);
    }

    fn apply(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            self.checker
                .on_effect(self.now, &effect, &self.core, &self.speech);
            self.react(&effect);
            self.trace.push((self.now, Entry::Effect(effect)));
        }
    }

    /// What the runtime would do with each effect.
    fn react(&mut self, effect: &Effect) {
        let now = self.now;
        match effect {
            Effect::Synthesize { clause, text } => {
                let ready = self.speech.synthesize(*clause, text);
                self.schedule(now.plus_ms(self.speech.synthesis_ms), Event::Input(ready));
            }
            Effect::StopVoice => {
                let last = self.speech.stop(now);
                self.schedule(
                    now.plus_ms(STOP_FADE_MS),
                    Event::Input(Input::VoiceStopped { last }),
                );
            }
            Effect::Duck { gain } => self.speech.set_gain(gain.value()),
            Effect::Talk { request } => {
                let inputs = self.talker.talk(request.clone());
                let generation = self.talker.generation();
                for (delay, input) in inputs {
                    self.schedule(now.plus_ms(delay), Event::Talker { generation, input });
                }
            }
            Effect::CancelTalk => self.talker.cancel(),
            Effect::RunTool { id, call } => {
                let output = self.tools.answer(call);
                let input = Input::ToolResult {
                    id: id.clone(),
                    output,
                };
                self.schedule(now.plus_ms(TOOL_LATENCY_MS), Event::Input(input));
            }
            Effect::RunHarness { task, spec } => {
                let (delay, event) = self.harness.run(spec.clone());
                let input = Input::Harness { task: *task, event };
                self.schedule(now.plus_ms(delay), Event::Input(input));
            }
            Effect::WakeAt { at } => self.schedule((*at).max(now), Event::Input(Input::Tick)),
            // The store's side of memory is tested in the runtime, against a real vault.
            Effect::Ui(_)
            | Effect::Log(_)
            | Effect::Remember(_)
            | Effect::CancelHarness { .. }
            | Effect::Forget { .. }
            | Effect::AcceptProposal { .. } => {}
        }
    }

    /// Keeps playback reports coming while there is audio to play.
    fn arm_speech(&mut self) {
        if self.speech_tick.is_none() && self.speech.is_active() {
            let at = self.now.plus_ms(PLAYBACK_REPORT_MS);
            self.speech_tick = Some(at);
            self.schedule(at, Event::Speech);
        }
    }
}
