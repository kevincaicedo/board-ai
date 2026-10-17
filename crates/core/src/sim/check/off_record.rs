//! I10 (core side) for sources (D-272): after "don't remember this session", nothing more of
//! the session is kept. No source is kept in a project, and no brief is sent to be cached: the
//! runtime saves a kept source's brief in the project's vault.

use crate::effect::{Effect, ForgetScope};
use crate::sources::SourceEffect;
use crate::time::SessionTime;

#[derive(Clone, Debug, Default)]
pub struct OffRecord {
    off: bool,
}

impl OffRecord {
    pub fn on_effect(&mut self, now: SessionTime, effect: &Effect, violations: &mut Vec<String>) {
        if let Effect::Forget {
            scope: ForgetScope::Session,
        } = effect
        {
            self.off = true;
            return;
        }
        let Effect::Source { source } = effect else {
            return;
        };
        let kept = match source {
            SourceEffect::CacheBrief { .. } => "a brief was sent to be cached",
            SourceEffect::Keep { .. } => "a source was kept in a project",
            SourceEffect::Resolve { .. }
            | SourceEffect::Share { .. }
            | SourceEffect::Discard { .. }
            | SourceEffect::SetProject { .. }
            | SourceEffect::Unkeep { .. }
            | SourceEffect::Brief { .. }
            | SourceEffect::CheckCitations { .. }
            | SourceEffect::ReadDocument { .. } => return,
        };
        if self.off {
            violations.push(format!(
                "{} ms: I10: {kept} after going off the record",
                now.as_ms()
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::OffRecord;
    use crate::effect::{Effect, ForgetScope};
    use crate::sources::{BriefTopic, SourceEffect, SourceId};
    use crate::time::SessionTime;

    fn cache() -> Effect {
        Effect::Source {
            source: SourceEffect::CacheBrief {
                source: SourceId(1),
                topic: BriefTopic::Overview,
                markdown: "# Brief: x\n## Overview\nO.\n".to_owned(),
            },
        }
    }

    /// The planted violation: a brief cached after "don't remember this session" turns it red;
    /// one cached before doesn't.
    #[test]
    fn a_brief_cached_off_the_record_is_a_violation() {
        let mut check = OffRecord::default();
        let mut violations = Vec::new();
        check.on_effect(SessionTime::ZERO, &cache(), &mut violations);
        assert!(violations.is_empty(), "{violations:?}");
        let forget = Effect::Forget {
            scope: ForgetScope::Session,
        };
        check.on_effect(SessionTime::ZERO, &forget, &mut violations);
        check.on_effect(SessionTime::ZERO, &cache(), &mut violations);
        assert_eq!(violations.len(), 1, "{violations:?}");
    }
}
