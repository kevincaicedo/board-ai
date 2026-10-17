//! Harness tasks at the session's end (D-144; review round 2): ending a session never stops a
//! running task silently. The orchestrator's rest kills whatever still runs, so by `SessionEnd`
//! the core must have cancelled each running task itself, which it does only once the user said
//! "end the session" again after hearing that it would stop the task.

use std::collections::BTreeSet;

use crate::harness::HarnessEvent;
use crate::ids::TaskId;
use crate::time::SessionTime;

#[derive(Clone, Debug, Default)]
pub struct Tasks {
    /// Started, and neither cancelled nor finished.
    running: BTreeSet<TaskId>,
}

impl Tasks {
    /// `RunHarness`.
    pub fn started(&mut self, task: TaskId) {
        self.running.insert(task);
    }

    /// `CancelHarness`.
    pub fn cancelled(&mut self, task: TaskId) {
        self.running.remove(&task);
    }

    /// A harness event came in; a finish or a failure ends the task.
    pub fn reported(&mut self, task: TaskId, event: &HarnessEvent) {
        match event {
            HarnessEvent::Finished { .. } | HarnessEvent::Failed { .. } => {
                self.running.remove(&task);
            }
            HarnessEvent::Progress { .. } | HarnessEvent::FileTouched { .. } => {}
        }
    }

    /// `SessionEnd`: nothing may still be running that the core never cancelled.
    pub fn on_session_end(&self, now: SessionTime, violations: &mut Vec<String>) {
        if self.running.is_empty() {
            return;
        }
        let tasks: Vec<String> = self.running.iter().map(|task| task.0.to_string()).collect();
        violations.push(format!(
            "{} ms: SessionEnd with harness tasks {} still running and never cancelled: the \
             session's end would stop them silently",
            now.as_ms(),
            tasks.join(", ")
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::Tasks;
    use crate::harness::{HarnessEvent, HarnessFailure};
    use crate::ids::TaskId;
    use crate::time::SessionTime;

    /// The planted violation: a session that ends over a running task is caught; one the core
    /// cancelled, or one that finished, is not.
    #[test]
    fn a_session_ending_over_a_running_task_is_caught() {
        let now = SessionTime::from_ms(1);
        let mut violations = Vec::new();
        let mut tasks = Tasks::default();
        tasks.started(TaskId(0));
        tasks.on_session_end(now, &mut violations);
        assert_eq!(violations.len(), 1, "{violations:?}");
        violations.clear();
        tasks.cancelled(TaskId(0));
        tasks.started(TaskId(1));
        let progress = HarnessEvent::Progress {
            text: "reading".to_owned(),
        };
        tasks.reported(TaskId(1), &progress);
        tasks.on_session_end(now, &mut violations);
        assert_eq!(violations.len(), 1, "progress ends nothing: {violations:?}");
        violations.clear();
        let failed = HarnessEvent::Failed {
            reason: HarnessFailure::Crashed,
            detail: "exit 1".to_owned(),
        };
        tasks.reported(TaskId(1), &failed);
        tasks.on_session_end(now, &mut violations);
        assert!(violations.is_empty(), "{violations:?}");
    }
}
