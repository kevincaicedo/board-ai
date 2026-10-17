//! The voice (LLD §2.4): clauses sent to text-to-speech, their word timings, and how far
//! playback has got. Everything that depends on "what has the user heard" reads it from here,
//! and it only moves forward on playback reports.

use std::collections::VecDeque;

use crate::ids::ClauseId;
use crate::input::{PlaybackPosition, WordTiming, WordTimings};
use crate::limits::ESTIMATED_SPEECH_MS_PER_BYTE;
use crate::narration::speakable::Speakable;
use crate::protocol::{Speaker, UiEvent};
use crate::time::SessionTime;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClauseSource {
    /// The talker's words from stream `stream` of the turn, starting at `caption_begin` bytes
    /// into that stream's speech.
    Talker { stream: usize, caption_begin: usize },
    /// A line the core speaks itself.
    Core,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    Synthesizing,
    Ready,
    Played,
    /// Synthesis failed: the clause never plays, so nothing anchored in it is drawn.
    Skipped,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoiceClause {
    pub id: ClauseId,
    pub caption: String,
    pub speakable: Speakable,
    pub source: ClauseSource,
    timings: Option<WordTimings>,
    status: Status,
    /// When the clause's audio began, from the latest playback report.
    started_at: Option<SessionTime>,
    revealed: usize,
}

impl VoiceClause {
    fn duration_ms(&self) -> u64 {
        self.timings.as_ref().map_or_else(
            || estimate_ms(self.speakable.text().len()),
            |timings| u64::from(timings.duration_ms),
        )
    }

    /// Byte offset of the last word that has started by `offset_ms`, or `None` before the first.
    fn started_word(&self, offset_ms: u32) -> Option<usize> {
        let timings = self.timings.as_ref()?;
        if offset_ms >= timings.duration_ms {
            return Some(self.speakable.text().len());
        }
        timings
            .words
            .iter()
            .take_while(|word| word.start_ms <= offset_ms)
            .last()
            .map(|word| usize::try_from(word.byte_offset).unwrap_or(usize::MAX))
    }

    /// The speakable offset reached at `offset_ms`, interpolated inside the current word, so a
    /// cut mid-word commits the part that was said ("checks Re").
    fn heard_speakable(&self, offset_ms: u32) -> usize {
        let length = self.speakable.text().len();
        let Some(timings) = self.timings.as_ref() else {
            return 0;
        };
        if offset_ms >= timings.duration_ms {
            return length;
        }
        let Some(index) = timings
            .words
            .iter()
            .rposition(|word| word.start_ms <= offset_ms)
        else {
            return 0;
        };
        let word = timings.words[index];
        let (next_offset, next_ms) =
            timings
                .words
                .get(index + 1)
                .map_or((length, timings.duration_ms), |next| {
                    (
                        usize::try_from(next.byte_offset).unwrap_or(length),
                        next.start_ms,
                    )
                });
        let begin = usize::try_from(word.byte_offset).unwrap_or(length);
        let span_ms = u64::from(next_ms.saturating_sub(word.start_ms).max(1));
        let into_ms = u64::from(offset_ms - word.start_ms);
        let span_bytes = u64::try_from(next_offset.saturating_sub(begin)).unwrap_or(0);
        let into = usize::try_from(span_bytes * into_ms / span_ms).unwrap_or(0);
        (begin + into).min(length)
    }

    /// Milliseconds into the clause's audio where speakable `offset` begins.
    fn ms_at(&self, offset: usize) -> u64 {
        let Some(timings) = self.timings.as_ref() else {
            return estimate_ms(offset);
        };
        if offset >= self.speakable.text().len() {
            return u64::from(timings.duration_ms);
        }
        timings
            .words
            .iter()
            .find(|word| usize::try_from(word.byte_offset).unwrap_or(usize::MAX) >= offset)
            .map_or(u64::from(timings.duration_ms), |word| {
                u64::from(word.start_ms)
            })
    }
}

fn estimate_ms(bytes: usize) -> u64 {
    u64::try_from(bytes).unwrap_or(u64::MAX) * u64::from(ESTIMATED_SPEECH_MS_PER_BYTE)
}

/// Sorts and clamps timings from the speech engine, so later code can rely on them (parse at
/// the boundary): offsets within the clause, start times never going backwards.
fn sanitize(mut timings: WordTimings, length: usize) -> WordTimings {
    let length = u32::try_from(length).unwrap_or(u32::MAX);
    timings.words.sort_by_key(|word| word.byte_offset);
    timings.words.retain(|word| word.byte_offset < length);
    timings.words.dedup_by_key(|word| word.byte_offset);
    let mut latest = 0;
    for word in &mut timings.words {
        latest = latest.max(word.start_ms).min(timings.duration_ms);
        *word = WordTiming {
            byte_offset: word.byte_offset,
            start_ms: latest,
        };
    }
    timings
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Voice {
    clauses: VecDeque<VoiceClause>,
    next_id: u32,
    heard: Option<PlaybackPosition>,
}

impl Voice {
    /// Queues a clause for synthesis; returns its id and the text to synthesize.
    pub fn add(&mut self, caption: String, speakable: Speakable, source: ClauseSource) -> ClauseId {
        let id = ClauseId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        self.clauses.push_back(VoiceClause {
            id,
            caption,
            speakable,
            source,
            timings: None,
            status: Status::Synthesizing,
            started_at: None,
            revealed: 0,
        });
        id
    }

    /// The caption of the last clause that played to its end.
    #[must_use]
    pub fn last_played_caption(&self) -> Option<&str> {
        self.clauses
            .iter()
            .rev()
            .find(|clause| clause.status == Status::Played)
            .map(|clause| clause.caption.as_str())
    }

    #[must_use]
    pub fn clause(&self, id: ClauseId) -> Option<&VoiceClause> {
        self.clauses.iter().find(|clause| clause.id == id)
    }

    /// Records a clause's timings; false for a clause this voice doesn't know (a stale report)
    /// or one already skipped.
    pub fn ready(&mut self, id: ClauseId, timings: WordTimings) -> bool {
        let Some(clause) = self
            .clauses
            .iter_mut()
            .find(|clause| clause.id == id && clause.status != Status::Skipped)
        else {
            return false;
        };
        clause.timings = Some(sanitize(timings, clause.speakable.text().len()));
        if clause.status == Status::Synthesizing {
            clause.status = Status::Ready;
        }
        true
    }

    /// Synthesis of a clause failed: it is skipped and will never play. Returns its caption, or
    /// `None` for a clause that is unknown or already past synthesis (a stale report).
    pub fn failed(&mut self, id: ClauseId) -> Option<&str> {
        let clause = self
            .clauses
            .iter_mut()
            .find(|clause| clause.id == id && clause.status == Status::Synthesizing)?;
        clause.status = Status::Skipped;
        Some(&clause.caption)
    }

    /// The clause was skipped: it will never play.
    #[must_use]
    pub fn skipped(&self, id: ClauseId) -> bool {
        self.clause(id)
            .is_some_and(|clause| clause.status == Status::Skipped)
    }

    /// The clause's audio has played to its end.
    #[must_use]
    pub fn played(&self, id: ClauseId) -> bool {
        self.clause(id)
            .is_some_and(|clause| clause.status == Status::Played)
    }

    /// A playback report. Earlier clauses have finished; positions never move backwards.
    pub fn playback_at(&mut self, now: SessionTime, position: PlaybackPosition) -> bool {
        let Some(index) = self
            .clauses
            .iter()
            .position(|clause| clause.id == position.clause)
        else {
            return false;
        };
        if self.clauses[index].timings.is_none() {
            return false;
        }
        if self.heard.is_some_and(|heard| {
            (heard.clause, heard.offset_ms) > (position.clause, position.offset_ms)
        }) {
            return false;
        }
        for clause in self.clauses.iter_mut().take(index) {
            if clause.status == Status::Ready {
                clause.status = Status::Played;
            }
        }
        let clause = &mut self.clauses[index];
        clause.started_at = Some(SessionTime::from_ms(
            now.as_ms().saturating_sub(u64::from(position.offset_ms)),
        ));
        if u64::from(position.offset_ms) >= clause.duration_ms() {
            clause.status = Status::Played;
        }
        self.heard = Some(position);
        true
    }

    /// Everything synthesized so far has played.
    pub fn drained(&mut self) {
        for clause in &mut self.clauses {
            if clause.status == Status::Ready {
                clause.status = Status::Played;
                let duration_ms = clause
                    .timings
                    .as_ref()
                    .map_or(0, |timings| timings.duration_ms);
                let end = PlaybackPosition {
                    clause: clause.id,
                    offset_ms: duration_ms,
                };
                if self.heard.is_none_or(|heard| heard.clause <= clause.id) {
                    self.heard = Some(end);
                }
            }
        }
    }

    /// True once the voice has reached speakable `offset` of `clause`: the word there has begun.
    #[must_use]
    pub fn reached(&self, clause: ClauseId, offset: usize) -> bool {
        let Some(target) = self.clause(clause) else {
            return false;
        };
        match target.status {
            Status::Played => return true,
            Status::Skipped => return false,
            Status::Synthesizing | Status::Ready => {}
        }
        let Some(heard) = self.heard.filter(|heard| heard.clause == clause) else {
            return false;
        };
        target
            .started_word(heard.offset_ms)
            .is_some_and(|spoken| spoken >= offset)
    }

    /// The furthest position known to have played.
    #[must_use]
    pub fn heard(&self) -> Option<PlaybackPosition> {
        self.heard
    }

    /// Every clause sent has been played or skipped.
    #[must_use]
    pub fn is_silent(&self) -> bool {
        self.clauses
            .iter()
            .all(|clause| matches!(clause.status, Status::Played | Status::Skipped))
    }

    /// Some clause has audio ready that hasn't finished playing.
    #[must_use]
    pub fn is_audible(&self) -> bool {
        self.clauses
            .iter()
            .any(|clause| clause.status == Status::Ready)
    }

    #[must_use]
    pub fn has_clauses(&self) -> bool {
        !self.clauses.is_empty()
    }

    /// Forgets this turn's clauses; late reports about them are ignored.
    pub fn clear(&mut self) {
        self.clauses.clear();
        self.heard = None;
    }

    /// How far into a talker stream's speech the user heard, given where playback stopped.
    /// Streams before the stopping clause were heard whole; streams after it, not at all.
    #[must_use]
    pub fn heard_in_stream(
        &self,
        position: Option<PlaybackPosition>,
        stream: usize,
        stream_caption_len: usize,
    ) -> usize {
        let Some(position) = position else {
            return 0;
        };
        let Some(index) = self
            .clauses
            .iter()
            .position(|clause| clause.id == position.clause)
        else {
            return 0;
        };
        let clause = &self.clauses[index];
        let (source_stream, caption_offset) =
            match clause.source {
                ClauseSource::Talker {
                    stream,
                    caption_begin,
                } => {
                    let speakable = clause.heard_speakable(position.offset_ms);
                    (
                        stream,
                        caption_begin + clause.speakable.caption_offset(speakable),
                    )
                }
                // A core line: the talker's words before it were heard whole.
                ClauseSource::Core => {
                    let previous = self.clauses.iter().take(index).rev().find_map(|clause| {
                        match clause.source {
                            ClauseSource::Talker {
                                stream,
                                caption_begin,
                            } => Some((stream, caption_begin + clause.caption.len())),
                            ClauseSource::Core => None,
                        }
                    });
                    let Some(previous) = previous else {
                        return 0;
                    };
                    previous
                }
            };
        match source_stream.cmp(&stream) {
            std::cmp::Ordering::Equal => caption_offset.min(stream_caption_len),
            std::cmp::Ordering::Greater => stream_caption_len,
            std::cmp::Ordering::Less => 0,
        }
    }

    /// Milliseconds until everything said so far has played, estimated as
    /// [`Self::estimate_ms_until`] is: a staged diagram's reveal draws while it is explained.
    #[must_use]
    pub fn estimate_ms_until_end(&self, now: SessionTime) -> u64 {
        self.clauses
            .iter()
            .map(|clause| match (clause.status, clause.started_at) {
                (Status::Played | Status::Skipped, _) => 0,
                (Status::Ready, Some(started_at)) => clause
                    .duration_ms()
                    .saturating_sub(now.ms_since(started_at)),
                (Status::Ready | Status::Synthesizing, None) | (Status::Synthesizing, Some(_)) => {
                    clause.duration_ms()
                }
            })
            .sum()
    }

    /// Milliseconds from `now` until speakable `offset` of `clause` plays, estimated from the
    /// timings known so far. Only op budgets use this; firing waits for real playback.
    #[must_use]
    pub fn estimate_ms_until(&self, clause: ClauseId, offset: usize, now: SessionTime) -> u64 {
        let mut before_ms = 0_u64;
        for candidate in &self.clauses {
            let played_ms = match (candidate.status, candidate.started_at) {
                (Status::Played | Status::Skipped, _) => {
                    if candidate.id == clause {
                        return 0;
                    }
                    continue;
                }
                (Status::Ready, Some(started_at)) => now.ms_since(started_at),
                (Status::Ready | Status::Synthesizing, None) | (Status::Synthesizing, Some(_)) => 0,
            };
            if candidate.id == clause {
                return before_ms + candidate.ms_at(offset).saturating_sub(played_ms);
            }
            before_ms += candidate.duration_ms().saturating_sub(played_ms);
        }
        before_ms
    }

    /// Caption updates since the last call: the playing clause up to the end of its current
    /// word, and a final caption for each clause that finished.
    pub fn captions(&mut self, events: &mut Vec<UiEvent>) {
        let heard = self.heard;
        for clause in &mut self.clauses {
            let target = match clause.status {
                Status::Played => clause.caption.len(),
                // A skipped clause's text is shown once, as a problem, by the engine.
                Status::Synthesizing | Status::Skipped => continue,
                Status::Ready => {
                    let Some(position) = heard.filter(|heard| heard.clause == clause.id) else {
                        continue;
                    };
                    let Some(word) = clause.started_word(position.offset_ms) else {
                        continue;
                    };
                    let next_word = clause.timings.as_ref().and_then(|timings| {
                        timings
                            .words
                            .iter()
                            .find(|candidate| {
                                usize::try_from(candidate.byte_offset).is_ok_and(|at| at > word)
                            })
                            .map(|candidate| usize::try_from(candidate.byte_offset).unwrap_or(word))
                    });
                    let end = next_word.unwrap_or(clause.speakable.text().len());
                    clause.speakable.caption_offset(end)
                }
            };
            let mut target = target.min(clause.caption.len());
            while !clause.caption.is_char_boundary(target) {
                target -= 1;
            }
            if target <= clause.revealed {
                continue;
            }
            clause.revealed = target;
            events.push(UiEvent::Caption {
                who: Speaker::Assistant,
                text: clause.caption[..target].trim_end().to_owned(),
                is_final: target == clause.caption.len(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ClauseSource, Voice};
    use crate::ids::ClauseId;
    use crate::input::{PlaybackPosition, TimingSource, WordTiming, WordTimings};
    use crate::narration::speakable::Speakable;
    use crate::time::SessionTime;

    fn timings(text: &str, ms_per_byte: u32) -> WordTimings {
        let mut words = Vec::new();
        let mut previous_space = true;
        for (offset, ch) in text.char_indices() {
            if previous_space && !ch.is_whitespace() {
                let offset = u32::try_from(offset).expect("short");
                words.push(WordTiming {
                    byte_offset: offset,
                    start_ms: offset * ms_per_byte,
                });
            }
            previous_space = ch.is_whitespace();
        }
        WordTimings {
            source: TimingSource::Estimated,
            words,
            duration_ms: u32::try_from(text.len()).expect("short") * ms_per_byte,
        }
    }

    fn voice_with(text: &str) -> (Voice, ClauseId) {
        let mut voice = Voice::default();
        let source = ClauseSource::Talker {
            stream: 0,
            caption_begin: 0,
        };
        let id = voice.add(text.to_owned(), Speakable::from_caption(text), source);
        assert!(voice.ready(id, timings(text, 10)));
        (voice, id)
    }

    #[test]
    fn an_anchor_is_reached_only_once_its_word_has_started() {
        let text = "Before Postgres it checks Redis";
        let (mut voice, id) = voice_with(text);
        let redis = text.find("Redis").expect("present");
        let at = |offset_ms| PlaybackPosition {
            clause: id,
            offset_ms,
        };
        assert!(voice.playback_at(SessionTime::from_ms(1_000), at(10 * 25)));
        assert!(!voice.reached(id, redis), "inside 'checks'");
        assert!(voice.playback_at(SessionTime::from_ms(1_010), at(10 * 26)));
        assert!(voice.reached(id, redis));
        assert!(
            !voice.playback_at(SessionTime::from_ms(1_020), at(5)),
            "never backwards"
        );
    }

    #[test]
    fn the_heard_offset_is_interpolated_inside_a_word() {
        let text = "Before Postgres it checks Redis";
        let (voice, id) = voice_with(text);
        let mid_redis = PlaybackPosition {
            clause: id,
            offset_ms: 10 * 28,
        };
        let heard = voice.heard_in_stream(Some(mid_redis), 0, text.len());
        assert_eq!(&text[..heard], "Before Postgres it checks Re");
        assert_eq!(voice.heard_in_stream(Some(mid_redis), 1, 50), 0);
        assert_eq!(voice.heard_in_stream(None, 0, text.len()), 0);
    }

    #[test]
    fn budgets_estimate_the_time_to_an_anchor() {
        let text = "one two three four";
        let (mut voice, id) = voice_with(text);
        let position = PlaybackPosition {
            clause: id,
            offset_ms: 40,
        };
        assert!(voice.playback_at(SessionTime::from_ms(500), position));
        let four = text.find("four").expect("present");
        let until = voice.estimate_ms_until(id, four, SessionTime::from_ms(500));
        assert_eq!(until, u64::try_from(four).expect("small") * 10 - 40);
    }
}
