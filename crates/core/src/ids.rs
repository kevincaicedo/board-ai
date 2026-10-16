//! Identifier newtypes. Each kind of id is its own type so they can't be mixed up.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::limits::{ELEMENT_ID_MAX_BYTES, TOOL_CALL_ID_MAX_BYTES};

/// A spoken clause, numbered in the order the core cut it from the narration.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS,
)]
#[serde(transparent)]
#[ts(export)]
pub struct ClauseId(pub u32);

/// A drawing op, numbered in narration order within the session.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS,
)]
#[serde(transparent)]
#[ts(export)]
pub struct OpId(pub u32);

/// A coding harness task.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS,
)]
#[serde(transparent)]
#[ts(export)]
pub struct TaskId(pub u32);

/// A surface beside the canvas (code card, compare, recap).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS,
)]
#[serde(transparent)]
#[ts(export)]
pub struct SurfaceId(pub u32);

/// Something the session captured (a memory, note, task or proposal), numbered by the core in
/// the order it sent them to the store, so "yes, keep it" can name the proposal it keeps.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct CaptureId(pub u32);

/// A board layer: 0 is the main board, and each open detour adds one (LLD §2.5).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS,
)]
#[serde(transparent)]
#[ts(export)]
pub struct LayerId(pub u8);

impl LayerId {
    pub const MAIN: Self = Self(0);
}

/// A board element's stable id, scoped to its layer. The talker chooses it (`<node id="lb"/>`),
/// so it is parsed at the boundary: 1 to [`ELEMENT_ID_MAX_BYTES`] bytes of ASCII letters, digits
/// and `_ - . : >`. The core builds edge ids as `from>to`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS)]
#[serde(try_from = "String", into = "String")]
#[ts(export, type = "string")]
pub struct ElementId(String);

impl ElementId {
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let valid_byte = |byte: u8| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':' | b'>')
        };
        let valid =
            !text.is_empty() && text.len() <= ELEMENT_ID_MAX_BYTES && text.bytes().all(valid_byte);
        valid.then(|| Self(text.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ElementId {
    type Error = String;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        Self::parse(&text).ok_or_else(|| format!("invalid element id: {text:?}"))
    }
}

impl From<ElementId> for String {
    fn from(id: ElementId) -> Self {
        id.0
    }
}

/// A provider's tool-call id, echoed back with the result. Opaque to the core; parsed so it is
/// bounded and printable.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ToolCallId(String);

impl ToolCallId {
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let valid = !text.is_empty()
            && text.len() <= TOOL_CALL_ID_MAX_BYTES
            && text.bytes().all(|byte| byte.is_ascii_graphic());
        valid.then(|| Self(text.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ToolCallId {
    type Error = String;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        Self::parse(&text).ok_or_else(|| format!("invalid tool call id: {text:?}"))
    }
}

impl From<ToolCallId> for String {
    fn from(id: ToolCallId) -> Self {
        id.0
    }
}

#[cfg(test)]
mod tests {
    use super::{ElementId, ToolCallId};
    use crate::limits::ELEMENT_ID_MAX_BYTES;

    #[test]
    fn element_ids_are_parsed_at_the_boundary() {
        for good in ["lb", "api-2", "cdn>lb", "a.b:c", "X_9"] {
            assert!(ElementId::parse(good).is_some(), "{good}");
        }
        let too_long = "a".repeat(ELEMENT_ID_MAX_BYTES + 1);
        for bad in ["", "has space", "quote\"", "ü", too_long.as_str()] {
            assert!(ElementId::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn deserializing_an_invalid_id_fails() {
        assert!(serde_json::from_str::<ElementId>("\"bad id\"").is_err());
        assert!(serde_json::from_str::<ToolCallId>("\"toolu_01\"").is_ok());
        assert!(serde_json::from_str::<ToolCallId>("\"\"").is_err());
    }
}
