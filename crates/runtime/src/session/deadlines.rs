//! The core's `WakeAt` deadlines (LLD §2.1). The orchestrator sleeps until the earliest and
//! steps one `Tick` for everything due: the core checks all of its deadlines on any tick.
//!
//! A deadline is kept until it is due, never fired early: the core asks once per distinct
//! deadline, so a tick before it would leave that deadline unanswered.

use std::collections::BTreeSet;

use boardai_core::time::SessionTime;

use crate::limits::SESSION_DEADLINES_MAX;

#[derive(Debug, Default)]
pub(crate) struct Deadlines {
    pending: BTreeSet<SessionTime>,
}

impl Deadlines {
    pub(crate) fn insert(&mut self, at: SessionTime) {
        if self.pending.len() >= SESSION_DEADLINES_MAX && !self.pending.contains(&at) {
            debug_assert!(
                false,
                "the core asked for over {SESSION_DEADLINES_MAX} deadlines"
            );
            tracing::error!(
                at_ms = at.as_ms(),
                "deadline dropped: {SESSION_DEADLINES_MAX} are already pending"
            );
            return;
        }
        self.pending.insert(at);
    }

    pub(crate) fn earliest(&self) -> Option<SessionTime> {
        self.pending.first().copied()
    }

    /// Forgets every deadline at or before `now`; true when there was one.
    pub(crate) fn take_due(&mut self, now: SessionTime) -> bool {
        let later = self.pending.split_off(&now.plus_ms(1));
        let due = !self.pending.is_empty();
        self.pending = later;
        due
    }

    pub(crate) fn clear(&mut self) {
        self.pending.clear();
    }
}

#[cfg(test)]
mod tests {
    use boardai_core::time::SessionTime;

    use super::Deadlines;

    fn at(ms: u64) -> SessionTime {
        SessionTime::from_ms(ms)
    }

    #[test]
    fn due_deadlines_go_together_and_later_ones_stay() {
        let mut deadlines = Deadlines::default();
        for ms in [900, 300, 300, 600] {
            deadlines.insert(at(ms));
        }
        assert_eq!(deadlines.earliest(), Some(at(300)));
        assert!(!deadlines.take_due(at(299)), "never early");
        assert!(deadlines.take_due(at(600)));
        assert_eq!(deadlines.earliest(), Some(at(900)));
        assert!(!deadlines.take_due(at(899)));
        deadlines.clear();
        assert_eq!(deadlines.earliest(), None);
    }
}
