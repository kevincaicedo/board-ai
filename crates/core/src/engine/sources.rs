//! Source chips (PRODUCT §7.3, D-123). When the talker says `path:line` for a line read this
//! session, a chip carrying that location appears as the voice reaches it; clicking it opens the
//! code card on that line. Both ends hold to law 6: a citation of anything not read shows no
//! chip (and the talker is told, when it names a repo path), and a click opens only lines read
//! this session, whatever the UI sends.

use crate::effect::{Dropped, Effect, LogRecord};
use crate::engine::Core;
use crate::limits::SOURCE_CHIPS_PER_REPLY_MAX;
use crate::protocol::{ChipKind, SourceRef, UiEvent};

/// Read lines a card opened from a chip shows above the cited line, and in all.
const SOURCE_LINES_BEFORE: u32 = 4;
const SOURCE_CARD_LINES: u32 = 14;

impl Core {
    /// The talker cited `path:line`. Shows its source chip once per reply, at most
    /// [`SOURCE_CHIPS_PER_REPLY_MAX`] a reply; false when there is nothing to show.
    pub(super) fn cite(&mut self, path: &str, line: u32, events: &mut Vec<UiEvent>) -> bool {
        let read = self.surfaces.read_files().resolve(path);
        let Some(read) = read.filter(|read| self.surfaces.read_files().has_line(read, line)) else {
            if path.contains('/') {
                self.context.notice(format!(
                    "You cited {path}:{line}, but that line was not read in this session, so no \
                     source chip was shown. Read it before saying anything about it."
                ));
            }
            return false;
        };
        let Some(reply) = self.reply.as_mut() else {
            return false;
        };
        let key = (read.clone(), line);
        if reply.cited.contains(&key) || reply.cited.len() >= SOURCE_CHIPS_PER_REPLY_MAX {
            return false;
        }
        reply.cited.insert(key);
        events.push(UiEvent::Chip {
            kind: ChipKind::Source,
            text: format!("{read}:{line}"),
            source: Some(SourceRef { path: read, line }),
        });
        true
    }

    /// A source chip was clicked: the code card opens on that line, highlighted, if it was read
    /// this session. Anything else the UI sends is dropped and counted.
    pub(super) fn open_source(&mut self, path: &str, line: u32, effects: &mut Vec<Effect>) {
        let files = self.surfaces.read_files();
        let range = files.resolve(path).and_then(|read| {
            Some((
                files.around(&read, line, SOURCE_LINES_BEFORE, SOURCE_CARD_LINES)?,
                read,
            ))
        });
        let mut events = Vec::new();
        // Whatever opens is shown: the core never holds a card the UI wasn't sent.
        let shown = range.is_some_and(|(range, read)| {
            let opened = self
                .surfaces
                .open_code(&read, Some(range), &mut events)
                .is_ok();
            if opened {
                let _ = self.surfaces.highlight(line, &mut events);
                self.context.notice(format!(
                    "The user opened {read}:{line} from a source chip; that code card is on \
                     screen."
                ));
            }
            opened
        });
        if !shown {
            effects.push(Effect::Log(LogRecord::Dropped {
                what: Dropped::Op,
                count: 1,
            }));
            return;
        }
        effects.extend(events.into_iter().map(Effect::Ui));
    }
}
