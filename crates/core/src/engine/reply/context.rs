//! What the talker was last told, and what waits for its next request: notices, harness reports
//! and the harness tasks still running.

use std::collections::{BTreeMap, VecDeque};

use crate::conversation::cues::CancelTarget;
use crate::harness::HarnessKind;
use crate::ids::TaskId;
use crate::limits::{HARNESS_REPORTS_MAX, TALKER_NOTICES_MAX};

/// What the talker was last told about the board and thread, plus notices and harness results
/// waiting for the next request.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::engine) struct ContextTracker {
    pub(super) board: String,
    pub(super) thread: String,
    pub(super) notices: VecDeque<String>,
    pub(super) reports: VecDeque<String>,
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

    pub(super) fn report(&mut self, text: String) {
        self.reports.push_back(text);
        // Past the limit the two oldest reports become one message, so nothing is lost.
        while self.reports.len() > HARNESS_REPORTS_MAX {
            let (Some(first), Some(second)) = (self.reports.pop_front(), self.reports.pop_front())
            else {
                break;
            };
            self.reports.push_front(format!("{first}\n\n{second}"));
        }
    }
}
