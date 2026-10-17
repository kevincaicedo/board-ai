//! A request started on an early final shows nothing until its turn is confirmed (D-262): no
//! speech, no BoardAI caption, no stroke, and the orb and the state stay listening. It is
//! confirmed when the turn's own request is marked (`TalkerRequested`) and gone when its stream
//! is cancelled.

use crate::conversation::ConversationState;
use crate::effect::{Effect, LogRecord, Milestone};
use crate::engine::Core;
use crate::protocol::{OrbState, Speaker, UiEvent};
use crate::time::SessionTime;

#[derive(Clone, Debug, Default)]
pub struct Speculation {
    /// When a request left on an early final that is not yet confirmed or cancelled.
    since: Option<SessionTime>,
}

impl Speculation {
    pub fn on_effect(&mut self, now: SessionTime, effect: &Effect, violations: &mut Vec<String>) {
        let shown = match effect {
            Effect::Log(LogRecord::Milestone {
                milestone: Milestone::SpeculativeRequest,
            }) => {
                self.since = Some(now);
                None
            }
            Effect::Log(LogRecord::Milestone {
                milestone: Milestone::TalkerRequested,
            })
            | Effect::CancelTalk => {
                self.since = None;
                None
            }
            Effect::Synthesize { text, .. } => Some(format!("spoke {text:?}")),
            Effect::Ui(UiEvent::Orb { state, .. }) if *state != OrbState::Listening => {
                Some(format!("the orb showed {state:?}"))
            }
            Effect::Ui(UiEvent::Caption {
                who: Speaker::Assistant,
                text,
                ..
            }) => Some(format!("captioned {text:?}")),
            Effect::Ui(UiEvent::OpStart { op, .. }) => Some(format!("drew {op:?}")),
            Effect::Talk { .. }
            | Effect::Duck { .. }
            | Effect::StopVoice
            | Effect::RunTool { .. }
            | Effect::RunHarness { .. }
            | Effect::CancelHarness { .. }
            | Effect::Ui(_)
            | Effect::Log(_)
            | Effect::Remember(_)
            | Effect::WakeAt { .. }
            | Effect::Forget { .. }
            | Effect::AcceptProposal { .. }
            | Effect::Source { .. } => None,
        };
        if let (Some(since), Some(shown)) = (self.since, shown) {
            violations.push(format!(
                "{} ms: D-262: {shown} while the request started at {} ms was unconfirmed",
                now.as_ms(),
                since.as_ms()
            ));
        }
    }

    /// After a step: an unconfirmed request means the user still holds the floor.
    pub fn after_step(&self, now: SessionTime, core: &Core, violations: &mut Vec<String>) {
        let Some(since) = self.since else {
            return;
        };
        if !matches!(core.state(), ConversationState::Listening(_)) {
            violations.push(format!(
                "{} ms: D-262: the core is {} with the request started at {} ms unconfirmed",
                now.as_ms(),
                core.state().name(),
                since.as_ms()
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Speculation;
    use crate::effect::{Effect, LogRecord, Milestone};
    use crate::ids::ClauseId;
    use crate::protocol::{OrbState, Speaker, UiEvent};
    use crate::time::SessionTime;

    fn mark(milestone: Milestone) -> Effect {
        Effect::Log(LogRecord::Milestone { milestone })
    }

    /// Planted violations: each thing shown while a speculative request is unconfirmed is
    /// caught, and nothing is once it is confirmed or cancelled.
    #[test]
    fn anything_shown_before_the_turn_is_confirmed_is_a_violation() {
        let shown = [
            Effect::Synthesize {
                clause: ClauseId(0),
                text: "Because".to_owned(),
            },
            Effect::Ui(UiEvent::Orb {
                state: OrbState::Thinking,
                level_percent: 0,
            }),
            Effect::Ui(UiEvent::Caption {
                who: Speaker::Assistant,
                text: "Because".to_owned(),
                is_final: false,
            }),
        ];
        for effect in &shown {
            for (settle, expected) in [
                (None, 1),
                (Some(mark(Milestone::TalkerRequested)), 0),
                (Some(Effect::CancelTalk), 0),
            ] {
                let mut check = Speculation::default();
                let mut violations = Vec::new();
                let at = SessionTime::from_ms(10);
                check.on_effect(at, &mark(Milestone::SpeculativeRequest), &mut violations);
                if let Some(settle) = &settle {
                    check.on_effect(at, settle, &mut violations);
                }
                check.on_effect(at, effect, &mut violations);
                assert_eq!(violations.len(), expected, "{effect:?} after {settle:?}");
            }
        }
    }
}
