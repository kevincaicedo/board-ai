//! What the talker was last told, and what waits for its next request: notices, harness reports
//! and the harness tasks still running.

use std::collections::{BTreeMap, VecDeque};

use crate::conversation::cues::CancelTarget;
use crate::harness::HarnessKind;
use crate::ids::TaskId;
use crate::limits::{HARNESS_REPORTS_MAX, TALKER_NOTICES_MAX};
use crate::sources::{BriefTopic, SourceId};

/// Harness or document text for a coming request, with what it gives the talker: a brief or a
/// digest is the talker's only once a request carries it (D-271).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::engine) struct Report {
    pub text: String,
    pub delivers: Vec<Delivery>,
}

/// A brief or a digest a report carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::engine) enum Delivery {
    Brief { source: SourceId, topic: BriefTopic },
    Digest { source: SourceId },
}

/// What the talker was last told about the board and thread, plus notices and harness results
/// waiting for the next request.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::engine) struct ContextTracker {
    pub(super) board: String,
    pub(super) thread: String,
    pub(super) notices: VecDeque<String>,
    pub(super) reports: VecDeque<Report>,
    /// Harness output nobody asked about yet (a prefetched brief, D-223, or a digest): it goes
    /// with the next new turn's request, fenced like a report, without a turn of its own, unless
    /// a call asks for it first (`take_quiet`).
    pub(super) quiet: VecDeque<Report>,
    pub(super) tasks: BTreeMap<TaskId, HarnessKind>,
}

impl ContextTracker {
    pub(in crate::engine) fn notice(&mut self, text: String) {
        self.notices.push_back(text);
        while self.notices.len() > TALKER_NOTICES_MAX {
            self.notices.pop_front();
        }
    }

    /// Forgets the running task a spoken cancel names (the newest of its kind) and returns it;
    /// whatever it reports afterwards is ignored.
    pub(in crate::engine) fn cancel_task(
        &mut self,
        target: CancelTarget,
    ) -> Option<(TaskId, HarnessKind)> {
        let task = self
            .tasks
            .iter()
            .rev()
            .find(|(_, kind)| match (target, kind) {
                (CancelTarget::Latest, _)
                | (CancelTarget::Research, HarnessKind::Research)
                | (CancelTarget::Review, HarnessKind::Review)
                | (CancelTarget::Change, HarnessKind::Change { .. }) => true,
                (
                    CancelTarget::Research | CancelTarget::Review | CancelTarget::Change,
                    HarnessKind::Research | HarnessKind::Review | HarnessKind::Change { .. },
                ) => false,
            })
            .map(|(task, _)| *task)?;
        self.tasks.remove_entry(&task)
    }

    /// The running task that ending the session would stop partway: a change first (it edits
    /// files), else the newest.
    pub(in crate::engine) fn running_task(&self) -> Option<&HarnessKind> {
        let change = self
            .tasks
            .values()
            .rev()
            .find(|kind| matches!(kind, HarnessKind::Change { .. }));
        change.or_else(|| self.tasks.values().next_back())
    }

    /// Forgets every running task and returns them, oldest first: they are being stopped, and
    /// whatever they report afterwards is ignored.
    pub(in crate::engine) fn stop_all(&mut self) -> Vec<(TaskId, HarnessKind)> {
        std::mem::take(&mut self.tasks).into_iter().collect()
    }

    pub(in crate::engine) fn report(&mut self, text: String, delivers: Vec<Delivery>) {
        push_merging(&mut self.reports, Report { text, delivers });
    }

    /// Harness output for the next new turn's request, with no turn of its own (a prefetched
    /// brief, D-223, or a digest).
    pub(in crate::engine) fn quiet_report(&mut self, text: String, delivers: Vec<Delivery>) {
        push_merging(&mut self.quiet, Report { text, delivers });
    }

    /// The waiting report that carries `delivery`, taken out to answer a call that asks for it
    /// now: it is then delivered once, with that call's result.
    pub(in crate::engine) fn take_quiet(&mut self, delivery: &Delivery) -> Option<Report> {
        let index = self
            .quiet
            .iter()
            .position(|report| report.delivers.contains(delivery))?;
        self.quiet.remove(index)
    }

    pub(in crate::engine) fn has_task(&self, task: TaskId) -> bool {
        self.tasks.contains_key(&task)
    }

    /// A task that ended outside the generic report (a brief, D-223).
    pub(in crate::engine) fn forget_task(&mut self, task: TaskId) {
        self.tasks.remove(&task);
    }
}

/// Past the limit the two oldest reports become one message, so nothing is lost.
fn push_merging(queue: &mut VecDeque<Report>, report: Report) {
    queue.push_back(report);
    while queue.len() > HARNESS_REPORTS_MAX {
        let (Some(first), Some(second)) = (queue.pop_front(), queue.pop_front()) else {
            break;
        };
        let mut delivers = first.delivers;
        delivers.extend(second.delivers);
        queue.push_front(Report {
            text: format!("{}\n\n{}", first.text, second.text),
            delivers,
        });
    }
}
