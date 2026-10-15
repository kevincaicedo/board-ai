//! Store identifiers, parsed at the boundary so a path can never be built from raw input.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::limits::SESSION_ID_MAX_BYTES;

/// A session's stable id. New sessions get `YYYY-MM-DD-HHMMSS`, plus `-N` when two start in
/// the same second. It is kept in the log's first line and the digest's front matter, so the
/// folder can be renamed after the title is known without breaking a link. A hand-made folder
/// without either file uses its folder name as its id.
///
/// 1 to [`SESSION_ID_MAX_BYTES`] bytes of ASCII letters, digits and `. _ -`, starting with a
/// letter or digit: no separators, so an id is always one path component and never `..`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SessionId(String);

impl SessionId {
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let valid_byte =
            |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-');
        let valid = text.len() <= SESSION_ID_MAX_BYTES
            && text
                .bytes()
                .next()
                .is_some_and(|first| first.is_ascii_alphanumeric())
            && text.bytes().all(valid_byte);
        valid.then(|| Self(text.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<String> for SessionId {
    type Error = String;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        Self::parse(&text).ok_or_else(|| "invalid session id".to_owned())
    }
}

impl From<SessionId> for String {
    fn from(id: SessionId) -> Self {
        id.0
    }
}

/// What a vault item is; its letter prefixes the item's id (`t27`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ItemKind {
    Note,
    Task,
    Memory,
    Proposal,
}

impl ItemKind {
    pub const ALL: [Self; 4] = [Self::Note, Self::Task, Self::Memory, Self::Proposal];

    const fn letter(self) -> char {
        match self {
            Self::Note => 'n',
            Self::Task => 't',
            Self::Memory => 'm',
            Self::Proposal => 'p',
        }
    }

    const fn position(self) -> usize {
        match self {
            Self::Note => 0,
            Self::Task => 1,
            Self::Memory => 2,
            Self::Proposal => 3,
        }
    }
}

/// A vault item's id, written at the end of its line as `^t27` (or in a note's front matter).
/// Numbers are unique per kind across the vault; the store hands out the next one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ItemId {
    kind: ItemKind,
    number: u32,
}

impl ItemId {
    #[must_use]
    pub const fn new(kind: ItemKind, number: u32) -> Self {
        Self { kind, number }
    }

    #[must_use]
    pub const fn kind(self) -> ItemKind {
        self.kind
    }

    #[must_use]
    pub const fn number(self) -> u32 {
        self.number
    }

    /// Parses `t27`; a leading `^` is not part of the id.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let mut chars = text.chars();
        let letter = chars.next()?;
        let kind = ItemKind::ALL
            .into_iter()
            .find(|kind| kind.letter() == letter)?;
        let digits = chars.as_str();
        let canonical = !digits.is_empty()
            && digits.bytes().all(|byte| byte.is_ascii_digit())
            && (digits == "0" || !digits.starts_with('0'));
        let number = canonical.then(|| digits.parse().ok())??;
        Some(Self { kind, number })
    }
}

impl fmt::Display for ItemId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}{}", self.kind.letter(), self.number)
    }
}

impl TryFrom<String> for ItemId {
    type Error = String;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        Self::parse(&text).ok_or_else(|| "invalid item id".to_owned())
    }
}

impl From<ItemId> for String {
    fn from(id: ItemId) -> Self {
        id.to_string()
    }
}

/// The next free number per kind. Seeded from the index after a scan, so ids stay unique
/// across restarts and hand edits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IdCounters {
    next: [u32; 4],
}

impl IdCounters {
    /// Makes sure the next id of this kind is above `seen`.
    pub fn observe(&mut self, id: ItemId) {
        let slot = &mut self.next[id.kind.position()];
        *slot = (*slot).max(id.number.saturating_add(1));
    }

    /// Hands out the next id. Numbers start at 1.
    pub fn take(&mut self, kind: ItemKind) -> ItemId {
        let slot = &mut self.next[kind.position()];
        let number = (*slot).max(1);
        *slot = number.saturating_add(1);
        ItemId { kind, number }
    }
}

#[cfg(test)]
mod tests {
    use super::{IdCounters, ItemId, ItemKind, SessionId};

    #[test]
    fn session_ids_are_one_safe_path_component() {
        for good in ["2026-10-15-143210", "2026-10-15-143210-2", "My_Session.v2"] {
            assert!(SessionId::parse(good).is_some(), "{good}");
        }
        let long = "a".repeat(97);
        for bad in [
            "",
            "..",
            ".hidden",
            "-x",
            "a/b",
            "a\\b",
            "a b",
            "ü",
            long.as_str(),
        ] {
            assert!(SessionId::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn item_ids_round_trip_and_reject_lookalikes() {
        for text in ["t27", "n1", "m0", "p4294967295"] {
            let id = ItemId::parse(text);
            assert_eq!(id.map(|id| id.to_string()), Some(text.to_owned()));
        }
        for bad in [
            "",
            "t",
            "x1",
            "t01",
            "t-1",
            "t1a",
            "^t1",
            "t4294967296",
            "T1",
        ] {
            assert_eq!(ItemId::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn counters_hand_out_ids_above_everything_seen() {
        let mut counters = IdCounters::default();
        assert_eq!(counters.take(ItemKind::Task).to_string(), "t1");
        counters.observe(ItemId::new(ItemKind::Task, 27));
        counters.observe(ItemId::new(ItemKind::Task, 3));
        assert_eq!(counters.take(ItemKind::Task).to_string(), "t28");
        assert_eq!(counters.take(ItemKind::Note).to_string(), "n1");
    }
}
