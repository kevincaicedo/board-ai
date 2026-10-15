//! The memory policy (PRODUCT §7.4, LLD §2.8, D-013): a pure function from what a memory is to
//! what happens to it. Secret redaction is not here; the store applies it at its boundary.

use crate::memory::{MemoryAction, MemoryKind};
use crate::tools::RememberKind;

/// One exhaustive match: a new kind of memory cannot be added without deciding its policy.
#[must_use]
pub const fn action(kind: MemoryKind) -> MemoryAction {
    match kind {
        MemoryKind::SessionRecord | MemoryKind::LearningSignal | MemoryKind::Stated => {
            MemoryAction::Keep
        }
        MemoryKind::Inferred => MemoryAction::Propose,
        MemoryKind::Note | MemoryKind::Task => MemoryAction::OnRequest,
        MemoryKind::RawAudio | MemoryKind::Secret => MemoryAction::Never,
    }
}

/// What the talker's `remember` tool asks for, as a memory kind.
#[must_use]
pub const fn remember_kind(kind: RememberKind) -> MemoryKind {
    match kind {
        RememberKind::Stated => MemoryKind::Stated,
        RememberKind::Inferred => MemoryKind::Inferred,
        RememberKind::Note => MemoryKind::Note,
    }
}

#[cfg(test)]
mod tests {
    use super::action;
    use crate::memory::{MemoryAction, MemoryKind};

    /// The PRODUCT §7.4 table, row by row.
    #[test]
    fn policy_table() {
        let table = [
            (MemoryKind::SessionRecord, MemoryAction::Keep),
            (MemoryKind::LearningSignal, MemoryAction::Keep),
            (MemoryKind::Stated, MemoryAction::Keep),
            (MemoryKind::Inferred, MemoryAction::Propose),
            (MemoryKind::Note, MemoryAction::OnRequest),
            (MemoryKind::Task, MemoryAction::OnRequest),
            (MemoryKind::RawAudio, MemoryAction::Never),
            (MemoryKind::Secret, MemoryAction::Never),
        ];
        for (kind, expected) in table {
            assert_eq!(action(kind), expected, "{kind:?}");
        }
    }
}
