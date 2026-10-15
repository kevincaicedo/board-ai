//! Where each clause sits on the played-frames counter, and what a counter value means.
//!
//! Clauses are written to the voice ring back to back, so clause `n` occupies frames
//! `[begin, end)` of the counter. A counter value `f` means frames `[0, f)` have been heard; it
//! falls in the clause with `begin < f <= end`, at `f - begin` frames in. At a boundary that is
//! the end of the earlier clause, never the start of the next: the next one has not been heard.

use std::collections::VecDeque;

use boardai_core::ids::ClauseId;
use boardai_core::input::PlaybackPosition;

use crate::audio::VOICE_RATE_HZ;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Entry {
    clause: ClauseId,
    begin_frame: u64,
    end_frame: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Timeline {
    entries: VecDeque<Entry>,
    end_frame: u64,
}

impl Timeline {
    /// Places a clause of `frames` frames right after the last one.
    pub(crate) fn append(&mut self, clause: ClauseId, frames: u64) {
        let begin_frame = self.end_frame;
        self.end_frame += frames;
        self.entries.push_back(Entry {
            clause,
            begin_frame,
            end_frame: self.end_frame,
        });
    }

    /// Forgets every clause; the next one starts at `frame`. After a stop the rest of the ring
    /// was dropped, so the counter continues from where playback actually stopped.
    pub(crate) fn reset(&mut self, frame: u64) {
        self.entries.clear();
        self.end_frame = frame;
    }

    /// The counter value at the end of the last clause.
    pub(crate) const fn end_frame(&self) -> u64 {
        self.end_frame
    }

    /// The position after `heard_frames` frames, or `None` when none of these clauses has been
    /// heard yet.
    pub(crate) fn position(&self, heard_frames: u64) -> Option<PlaybackPosition> {
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.begin_frame < heard_frames && heard_frames <= entry.end_frame)?;
        let offset_frames = heard_frames - entry.begin_frame;
        let offset_ms = offset_frames * 1_000 / u64::from(VOICE_RATE_HZ);
        Some(PlaybackPosition {
            clause: entry.clause,
            offset_ms: u32::try_from(offset_ms).unwrap_or(u32::MAX),
        })
    }

    /// Drops clauses that ended before `heard_frames`; the one that holds it stays.
    pub(crate) fn retire(&mut self, heard_frames: u64) {
        while self
            .entries
            .front()
            .is_some_and(|entry| entry.end_frame < heard_frames)
        {
            self.entries.pop_front();
        }
    }

    /// Clauses not completely heard.
    pub(crate) fn unheard_clauses(&self, heard_frames: u64) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.end_frame > heard_frames)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use boardai_core::ids::ClauseId;
    use boardai_core::input::PlaybackPosition;

    use super::Timeline;

    fn at(clause: u32, offset_ms: u32) -> Option<PlaybackPosition> {
        Some(PlaybackPosition {
            clause: ClauseId(clause),
            offset_ms,
        })
    }

    #[test]
    fn counter_values_map_to_clause_offsets() {
        let mut timeline = Timeline::default();
        timeline.append(ClauseId(1), 24_000);
        timeline.append(ClauseId(2), 12_000);
        assert_eq!(timeline.position(0), None);
        assert_eq!(timeline.position(2_400), at(1, 100));
        // The boundary belongs to the clause that ends there.
        assert_eq!(timeline.position(24_000), at(1, 1_000));
        assert_eq!(timeline.position(24_024), at(2, 1));
        assert_eq!(timeline.position(36_000), at(2, 500));
        assert_eq!(timeline.position(36_001), None);
        assert_eq!(timeline.unheard_clauses(24_000), 1);
    }

    #[test]
    fn a_reset_continues_from_where_playback_stopped() {
        let mut timeline = Timeline::default();
        timeline.append(ClauseId(1), 24_000);
        timeline.reset(5_000);
        assert_eq!(timeline.position(5_000), None);
        assert_eq!(timeline.unheard_clauses(0), 0);
        timeline.append(ClauseId(2), 2_400);
        assert_eq!(timeline.position(5_000), None);
        assert_eq!(timeline.position(7_400), at(2, 100));
        assert_eq!(timeline.end_frame(), 7_400);
    }

    #[test]
    fn retiring_keeps_the_clause_being_heard() {
        let mut timeline = Timeline::default();
        timeline.append(ClauseId(1), 100);
        timeline.append(ClauseId(2), 100);
        timeline.retire(100);
        assert_eq!(timeline.position(100), at(1, 4));
        timeline.retire(101);
        assert_eq!(timeline.position(100), None);
        assert_eq!(timeline.position(150), at(2, 2));
    }
}
