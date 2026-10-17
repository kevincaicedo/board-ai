//! Research briefs in the conversation (D-185).
//!
//! - **Routing.** While a code source is shared, a `research` call about it is the core's to
//!   answer: from a brief the talker already has, from one waiting to go with the next turn
//!   (taken out and given to the call), from the cache at once (`BriefCached`), or by starting
//!   the harness on the brief request (`BriefStarted`). The talker never waits long: a running
//!   brief answers "still reading". Research with no code source shared is unchanged.
//! - **Held calls.** A call about a brief the cache or the citation check is about to answer is
//!   held with any other call about it (models call tools in parallel): all of them are answered
//!   when the harness starts, when the brief arrives, and when its source stops being shared, so
//!   a tool round never waits for nothing (D-268).
//! - **Arrival.** The brief is parsed into a bounded [`Brief`]; every citation is read natively
//!   (`CheckCitations`) before any chip may show it, and an unread one is removed and the
//!   talker told (law 6, D-123). The brief goes to the talker fenced as untrusted harness output
//!   (D-107): as the research call's result, as a report turn for a question asked earlier, or
//!   quietly with the next request for a prefetch nobody asked about yet. The talker has it only
//!   once a request carries it (D-271). Off the record, it is never sent to be cached (I10).
//! - **Progress.** Files the harness reads become the tray's progress and the ghost names.

mod text;

use std::collections::BTreeSet;

use crate::effect::Effect;
use crate::engine::Core;
use crate::engine::reply::Delivery;
use crate::engine::shared::diagrams::Carried;
use crate::harness::{HarnessEvent, HarnessFailure, HarnessKind};
use crate::history::UserPart;
use crate::ids::{TaskId, ToolCallId};
use crate::limits::NAME_KEY_MIN_CHARS;
use crate::sources::brief::{self, Brief, request_prompt};
use crate::sources::state::reason;
use crate::sources::{
    BriefTopic, Citation, CitedLines, SourceEffect, SourceId, SourceState, name_key,
};
use crate::time::SessionTime;
use text::{BriefReport, already_here, answered_with_other, brief_report, failed, still_reading};

/// Who is waiting for a brief.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::engine) enum Waiting {
    /// A prefetch: it goes to the talker quietly with the next request.
    Nobody,
    /// `research` calls hold their results open for it, in call order (never empty): the first
    /// gets the brief, any other is pointed at that result.
    Calls(Vec<ToolCallId>),
    /// The user asked: it gets a report turn of its own once the floor is free.
    Talker,
}

#[derive(Clone, Debug)]
enum Phase {
    /// Asked of the runtime: the cache, or the harness.
    Asked,
    /// The harness is reading.
    Running,
    /// Parsed; its citations are being read.
    Checking { brief: Box<Brief>, cached: bool },
}

#[derive(Clone, Debug)]
pub(in crate::engine) struct BriefTask {
    pub source: SourceId,
    pub topic: BriefTopic,
    pub waiting: Waiting,
    phase: Phase,
    /// The harness's latest progress note ("Reading the arena first."), so "still reading" can
    /// say what it is looking at.
    note: Option<String>,
    /// Files the harness read for this brief (each once), for its diagram's chip (D-242).
    pub files_read: u32,
}

impl Core {
    /// Asks the runtime for a brief: the cache's, or research on the request.
    pub(in crate::engine) fn request_brief(
        &mut self,
        source: SourceId,
        topic: BriefTopic,
        question: Option<&str>,
        waiting: Waiting,
        effects: &mut Vec<Effect>,
    ) -> Option<TaskId> {
        let entry = self.sources.shared.get(source)?;
        let prompt = request_prompt(&entry.info.title, entry.info.kind, &topic, question);
        let task = self.start_task(HarnessKind::Research);
        self.sources.briefs.insert(
            task,
            BriefTask {
                source,
                topic: topic.clone(),
                waiting,
                phase: Phase::Asked,
                note: None,
                files_read: 0,
            },
        );
        effects.push(Effect::Source {
            source: SourceEffect::Brief {
                task,
                source,
                topic,
                prompt,
            },
        });
        Some(task)
    }

    /// A brief about `source` on `topic` already on its way.
    pub(in crate::engine) fn brief_running(
        &self,
        source: SourceId,
        topic: &BriefTopic,
    ) -> Option<TaskId> {
        self.sources
            .briefs
            .iter()
            .find(|(_, task)| task.source == source && task.topic == *topic)
            .map(|(task, _)| *task)
    }

    /// A component brief on its way about the part called `id` or `label`: the harness that
    /// reads it, whose diagram will be staged on that part's detail layer (D-242).
    pub(in crate::engine) fn brief_coming_for(&self, id: &str, label: &str) -> Option<String> {
        let keys = [name_key(id), name_key(label)];
        let coming = self
            .sources
            .briefs
            .values()
            .any(|brief| match &brief.topic {
                BriefTopic::Component { name } => keys.contains(&name_key(name)),
                BriefTopic::Overview => false,
            });
        coming.then(|| self.sources.shared.harness_name().to_owned())
    }

    /// The topic a question asks about: a part of the overview it names, else the overview.
    fn topic_of(&self, source: SourceId, question: &str) -> BriefTopic {
        let key = name_key(question);
        let Some(entry) = self.sources.shared.get(source) else {
            return BriefTopic::Overview;
        };
        entry
            .components
            .iter()
            .filter(|(_, name)| {
                name_key(name).len() >= NAME_KEY_MIN_CHARS && key.contains(&name_key(name))
            })
            .max_by_key(|(_, name)| name.len())
            .map_or(BriefTopic::Overview, |(_, name)| {
                BriefTopic::component(name)
            })
    }

    /// The `research` call while a code source is shared: `None` when it isn't about one (the
    /// call runs as before), `Some(None)` when its result is held for a brief on its way, and
    /// `Some(Some(result))` when the core answers now.
    pub(in crate::engine) fn research_on_source(
        &mut self,
        id: &ToolCallId,
        question: &str,
        effects: &mut Vec<Effect>,
    ) -> Option<Option<UserPart>> {
        let source = self.sources.shared.code_source_for(question)?;
        let title = self.sources.shared.get(source)?.info.title.clone();
        let harness = self.sources.shared.harness_name().to_owned();
        let answer = |content: String| {
            Some(Some(UserPart::ToolResult {
                id: id.clone(),
                content,
                is_error: false,
            }))
        };
        if !self.sources.shared.has_harness() {
            return answer(format!(
                "No coding harness is installed, so {title} can't be researched. Read what you \
                 need with read_file, and say so."
            ));
        }
        let topic = self.topic_of(source, question);
        if let Some(task) = self.brief_running(source, &topic) {
            return self
                .research_while_running(id, task)
                .map_or(Some(None), &answer);
        }
        let delivered = self
            .sources
            .shared
            .get(source)
            .is_some_and(|entry| entry.delivered.contains(&topic));
        if delivered {
            let staged = self.diagram_in_use(source, &topic);
            return answer(already_here(&harness, &topic, &title, staged));
        }
        // A prefetch still waiting for the next turn: this call carries it instead (D-271).
        let delivery = Delivery::Brief {
            source,
            topic: topic.clone(),
        };
        if let Some(report) = self.context.take_quiet(&delivery) {
            let _ = self.diagram_in_use(source, &topic);
            return Some(Some(self.held_result(id, report.text, report.delivers)));
        }
        let waiting = Waiting::Calls(vec![id.clone()]);
        self.request_brief(source, topic, Some(question), waiting, effects)
            .map(|_| None)
    }

    /// A `research` call about a brief on its way. Asked of the runtime or having its citations
    /// read, it answers in moments: the call waits with any other held for it (`None`). The
    /// harness reading: "still reading" now, and a report turn once it is here.
    fn research_while_running(&mut self, id: &ToolCallId, task: TaskId) -> Option<String> {
        let harness = self.sources.shared.harness_name().to_owned();
        let brief = self.sources.briefs.get_mut(&task)?;
        let soon = matches!(brief.phase, Phase::Asked | Phase::Checking { .. });
        match &mut brief.waiting {
            Waiting::Calls(held) => {
                held.push(id.clone());
                return None;
            }
            waiting @ (Waiting::Nobody | Waiting::Talker) if soon => {
                *waiting = Waiting::Calls(vec![id.clone()]);
                return None;
            }
            waiting @ (Waiting::Nobody | Waiting::Talker) => *waiting = Waiting::Talker,
        }
        let (source, topic, note) = (brief.source, brief.topic.clone(), brief.note.clone());
        let title = self
            .sources
            .shared
            .get(source)
            .map(|entry| entry.info.title.clone())
            .unwrap_or_default();
        Some(still_reading(
            &harness,
            &title,
            &topic,
            task,
            note.as_deref(),
        ))
    }

    /// Harness events of a brief task; false for any other task.
    pub(in crate::engine) fn on_brief_harness(
        &mut self,
        now: SessionTime,
        task: TaskId,
        event: &HarnessEvent,
        effects: &mut Vec<Effect>,
    ) -> bool {
        let Some(source) = self.sources.briefs.get(&task).map(|brief| brief.source) else {
            return false;
        };
        if self.has_ended() {
            // The session is over: nothing it started speaks or shows again.
            return true;
        }
        if !self.context.has_task(task) {
            // Cancelled by voice: whatever it reports is ignored.
            self.sources.briefs.remove(&task);
            self.sources.shared.finish_reading(source, None);
            self.show_tray(effects);
            return true;
        }
        match event {
            HarnessEvent::Progress { text } => {
                if let Some(brief) = self.sources.briefs.get_mut(&task) {
                    brief.note = Some(reason(text));
                }
                return true;
            }
            HarnessEvent::FileTouched { path } => {
                if let Some(brief) = self.sources.briefs.get_mut(&task) {
                    brief.files_read = brief.files_read.saturating_add(1);
                }
                self.sources.shared.read_file(source, path);
            }
            HarnessEvent::Finished { summary, .. } => {
                self.context.forget_task(task);
                self.on_brief_arrived(now, task, summary, false, effects);
            }
            HarnessEvent::Failed { reason, detail } => {
                self.context.forget_task(task);
                self.brief_failed(now, task, *reason, detail, effects);
            }
        }
        self.show_tray(effects);
        true
    }

    /// `BriefStarted`: the harness reads; every held research call learns so at once.
    pub(in crate::engine) fn on_brief_started(
        &mut self,
        now: SessionTime,
        task: TaskId,
        effects: &mut Vec<Effect>,
    ) {
        let Some(brief) = self.sources.briefs.get_mut(&task) else {
            return;
        };
        brief.phase = Phase::Running;
        let (source, topic) = (brief.source, brief.topic.clone());
        let held = match std::mem::replace(&mut brief.waiting, Waiting::Talker) {
            Waiting::Calls(held) => held,
            waiting @ (Waiting::Nobody | Waiting::Talker) => {
                brief.waiting = waiting;
                Vec::new()
            }
        };
        self.sources.shared.start_reading(source);
        if held.is_empty() {
            return;
        }
        let title = self
            .sources
            .shared
            .get(source)
            .map(|entry| entry.info.title.clone())
            .unwrap_or_default();
        let harness = self.sources.shared.harness_name().to_owned();
        let text = still_reading(&harness, &title, &topic, task, None);
        let mut answered = false;
        for id in &held {
            answered |= self.answer_held_call(now, id, text.clone(), Vec::new(), effects);
        }
        if !answered && let Some(brief) = self.sources.briefs.get_mut(&task) {
            brief.waiting = Waiting::Nobody;
        }
    }

    /// `BriefCached`.
    pub(in crate::engine) fn on_brief_text(
        &mut self,
        now: SessionTime,
        task: TaskId,
        markdown: &str,
        effects: &mut Vec<Effect>,
    ) {
        self.context.forget_task(task);
        self.on_brief_arrived(now, task, markdown, true, effects);
    }

    /// A brief's text arrived: parse it, then read its citations before anything else.
    fn on_brief_arrived(
        &mut self,
        now: SessionTime,
        task: TaskId,
        markdown: &str,
        cached: bool,
        effects: &mut Vec<Effect>,
    ) {
        let parsed = brief::parse(markdown);
        if !parsed.is_usable() {
            let detail = "its answer wasn't a brief".to_owned();
            self.brief_failed(now, task, HarnessFailure::BadOutput, &detail, effects);
            return;
        }
        let Some(brief_task) = self.sources.briefs.get_mut(&task) else {
            return;
        };
        let citations = parsed.all_citations();
        let source = brief_task.source;
        brief_task.phase = Phase::Checking {
            brief: Box::new(parsed),
            cached,
        };
        if citations.is_empty() {
            self.on_citations_checked(now, task, Vec::new(), &[], effects);
            return;
        }
        effects.push(Effect::Source {
            source: SourceEffect::CheckCitations {
                task,
                source,
                citations,
            },
        });
    }

    /// `CitationsChecked`: what was read is recorded as read this session (its chips may show);
    /// every other citation leaves the brief before the talker sees it.
    pub(in crate::engine) fn on_citations_checked(
        &mut self,
        now: SessionTime,
        task: TaskId,
        read: Vec<CitedLines>,
        unverified: &[Citation],
        effects: &mut Vec<Effect>,
    ) {
        let Some(brief_task) = self.sources.briefs.remove(&task) else {
            return;
        };
        let Phase::Checking { mut brief, cached } = brief_task.phase.clone() else {
            self.sources.briefs.insert(task, brief_task);
            return;
        };
        let covered = |citation: &Citation| {
            !unverified.contains(citation)
                && read.iter().any(|lines| {
                    let count = u64::try_from(lines.lines.len()).unwrap_or(u64::MAX);
                    let last = u64::from(lines.first_line).saturating_add(count);
                    lines.path == citation.path
                        && lines.first_line <= citation.first_line
                        && u64::from(citation.last_line) < last
                })
        };
        let verified: BTreeSet<Citation> = brief
            .all_citations()
            .into_iter()
            .filter(|citation| covered(citation))
            .collect();
        for lines in &read {
            self.surfaces
                .record_read(&lines.path, lines.first_line, &lines.lines);
        }
        let removed = brief.keep_citations(&verified);
        let checked = Checked { removed, cached };
        self.deliver_brief(now, task, &brief_task, &brief, checked, effects);
    }

    fn deliver_brief(
        &mut self,
        now: SessionTime,
        task: TaskId,
        brief_task: &BriefTask,
        brief: &Brief,
        checked: Checked,
        effects: &mut Vec<Effect>,
    ) {
        let Checked { removed, cached } = checked;
        let source = brief_task.source;
        let harness = self.sources.shared.harness_name().to_owned();
        let settled = (brief_task.topic == BriefTopic::Overview).then(|| {
            let parts = brief.components.len();
            if removed > 0 {
                SourceState::Partial {
                    reason: reason(&format!(
                        "Overview ready · {parts} parts mapped · {removed} citations couldn't be \
                         checked"
                    )),
                }
            } else {
                SourceState::Ready {
                    detail: format!("Overview ready · {parts} parts mapped"),
                }
            }
        });
        let Some(entry) = self.sources.shared.get_mut(source) else {
            return;
        };
        if brief_task.topic == BriefTopic::Overview {
            entry.components = brief
                .components
                .iter()
                .map(|part| (part.id.as_str().to_owned(), part.name.clone()))
                .collect();
        }
        let title = entry.info.title.clone();
        self.sources.shared.finish_reading(source, settled);
        let markdown = brief.to_markdown();
        // Off the record nothing of the session is kept, a brief included (I10).
        if !cached && !self.memory.is_off_record() {
            effects.push(Effect::Source {
                source: SourceEffect::CacheBrief {
                    source,
                    topic: brief_task.topic.clone(),
                    markdown: markdown.clone(),
                },
            });
        }
        let carried = self.carried_by(&brief_task.waiting);
        let diagram = self.wait_to_stage(brief_task, brief, cached, carried, effects);
        let report = brief_report(&BriefReport {
            task,
            harness: &harness,
            title: &title,
            topic: &brief_task.topic,
            markdown: &markdown,
            removed,
            cached,
            diagram,
        });
        let delivery = Delivery::Brief {
            source,
            topic: brief_task.topic.clone(),
        };
        self.hand_over(now, &brief_task.waiting, report, Some(delivery), effects);
        self.show_tray(effects);
    }

    /// When a brief's diagram goes on the board: with the request that carries the brief, or,
    /// for a prefetch that goes quietly with whatever is asked next, once the talker uses it
    /// (D-242). A held call's goes with its reply's tool round (D-270).
    fn carried_by(&self, waiting: &Waiting) -> Carried {
        match waiting {
            Waiting::Calls(held) if held.iter().any(|id| self.holds_call(id)) => Carried::ThisReply,
            Waiting::Talker => Carried::NextRequest,
            Waiting::Calls(_) | Waiting::Nobody => Carried::OnUse,
        }
    }

    /// The talker gets a brief or a failure: as the held calls' results (the first gets it, the
    /// rest are pointed at it), as its own report turn, or quietly with the next request.
    fn hand_over(
        &mut self,
        now: SessionTime,
        waiting: &Waiting,
        text: String,
        delivery: Option<Delivery>,
        effects: &mut Vec<Effect>,
    ) {
        let delivers: Vec<Delivery> = delivery.into_iter().collect();
        match waiting {
            Waiting::Calls(held) => {
                let mut answered = false;
                for id in held {
                    let (content, carries) = if answered {
                        (answered_with_other(), Vec::new())
                    } else {
                        (text.clone(), delivers.clone())
                    };
                    answered |= self.answer_held_call(now, id, content, carries, effects);
                }
                if !answered {
                    self.context.quiet_report(text, delivers);
                }
            }
            Waiting::Talker => self.context.report(text, delivers),
            Waiting::Nobody => self.context.quiet_report(text, delivers),
        }
    }

    fn brief_failed(
        &mut self,
        now: SessionTime,
        task: TaskId,
        failure: HarnessFailure,
        detail: &str,
        effects: &mut Vec<Effect>,
    ) {
        let Some(brief_task) = self.sources.briefs.remove(&task) else {
            return;
        };
        let harness = self.sources.shared.harness_name().to_owned();
        let title = self
            .sources
            .shared
            .get(brief_task.source)
            .map(|entry| entry.info.title.clone())
            .unwrap_or_default();
        let why = failure_words(failure);
        let settled = (brief_task.topic == BriefTopic::Overview).then(|| SourceState::Failed {
            reason: reason(&format!("{harness} couldn't read it: {why}")),
        });
        self.sources
            .shared
            .finish_reading(brief_task.source, settled);
        let text = failed(&harness, &brief_task.topic, &title, task, why, detail);
        self.hand_over(now, &brief_task.waiting, text, None, effects);
        self.show_tray(effects);
    }

    /// Briefs about sources no longer shared (removed from the tray, or their project left):
    /// each task is cancelled, and every research call held for one is answered now, so its
    /// reply goes on (D-268). The talker hears once of a brief it was promised.
    pub(in crate::engine) fn drop_briefs_of(
        &mut self,
        now: SessionTime,
        sources: &[SourceId],
        effects: &mut Vec<Effect>,
    ) {
        let gone: Vec<TaskId> = self
            .sources
            .briefs
            .iter()
            .filter(|(_, task)| sources.contains(&task.source))
            .map(|(task, _)| *task)
            .collect();
        let mut promised = false;
        for task in gone {
            let Some(brief) = self.sources.briefs.remove(&task) else {
                continue;
            };
            self.context.forget_task(task);
            effects.push(Effect::CancelHarness { task });
            match brief.waiting {
                Waiting::Calls(held) => {
                    for id in &held {
                        let _ = self.answer_held_call(now, id, text::gone(), Vec::new(), effects);
                    }
                }
                Waiting::Talker => promised = true,
                Waiting::Nobody => {}
            }
        }
        if promised {
            self.context.notice(text::gone());
        }
    }

    /// What a report or a held call's result carried reached the talker: it is in the history.
    pub(in crate::engine) fn delivered(&mut self, delivers: &[Delivery]) {
        for delivery in delivers {
            match delivery {
                Delivery::Brief { source, topic } => {
                    if let Some(entry) = self.sources.shared.get_mut(*source) {
                        entry.deliver(topic.clone());
                    }
                }
                Delivery::Digest { source } => self.sources.documents.delivered(*source),
            }
        }
    }

    /// A held call answered now, with what its result carries.
    fn held_result(
        &mut self,
        id: &ToolCallId,
        content: String,
        delivers: Vec<Delivery>,
    ) -> UserPart {
        self.carry_with_call(id, delivers);
        UserPart::ToolResult {
            id: id.clone(),
            content,
            is_error: false,
        }
    }
}

/// What reading a brief's citations found, and where the brief came from.
#[derive(Clone, Copy)]
struct Checked {
    removed: usize,
    cached: bool,
}

fn failure_words(failure: HarnessFailure) -> &'static str {
    match failure {
        HarnessFailure::NotInstalled => "no coding harness is installed",
        HarnessFailure::TimedOut => "it ran out of time",
        HarnessFailure::Cancelled => "it was cancelled",
        HarnessFailure::Crashed => "it stopped unexpectedly",
        HarnessFailure::BadOutput => "its answer wasn't a brief",
        HarnessFailure::QueueFull => "too many tasks are running",
    }
}
