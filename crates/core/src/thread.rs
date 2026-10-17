//! The thread (LLD §2.6): the explanation plan, the current step, and a bounded stack of
//! detours. Each detour owns a board layer.

use crate::ids::LayerId;
use crate::limits::DETOUR_DEPTH_MAX;
use crate::narration::ops::StepRef;
use crate::protocol::{DetailAnchor, UiEvent};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detour {
    pub title: String,
    pub layer: LayerId,
    /// A detail layer's place on the layer it drills into (D-188); `None` for a detour.
    pub anchor: Option<DetailAnchor>,
}

/// At most [`DETOUR_DEPTH_MAX`] detours; a push past it is refused, never stored (I4).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DetourStack {
    items: Vec<Detour>,
}

impl DetourStack {
    /// Refuses the detour, handing it back, when the stack is full.
    pub fn push(&mut self, detour: Detour) -> Result<(), Detour> {
        if self.items.len() >= DETOUR_DEPTH_MAX {
            return Err(detour);
        }
        self.items.push(detour);
        Ok(())
    }

    pub fn pop(&mut self) -> Option<Detour> {
        self.items.pop()
    }

    #[must_use]
    pub fn depth(&self) -> usize {
        self.items.len()
    }

    #[must_use]
    pub fn top(&self) -> Option<&Detour> {
        self.items.last()
    }

    /// Outermost first.
    #[must_use]
    pub fn items(&self) -> &[Detour] {
        &self.items
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Thread {
    steps: Vec<String>,
    /// A 0-based index into `steps`.
    current: Option<u8>,
    detours: DetourStack,
}

impl Thread {
    pub fn set_plan(&mut self, steps: Vec<String>) {
        self.steps = steps;
        self.current = None;
    }

    /// Moves to a step by number (1-based) or name; false when there is no such step.
    pub fn set_step(&mut self, step: &StepRef) -> bool {
        let index = match step {
            StepRef::Number(number) => usize::from(*number).checked_sub(1),
            StepRef::Name(name) => self
                .steps
                .iter()
                .position(|step| step.eq_ignore_ascii_case(name)),
        };
        let Some(index) = index.filter(|index| *index < self.steps.len()) else {
            return false;
        };
        self.current = u8::try_from(index).ok();
        self.current.is_some()
    }

    pub fn push_detour(&mut self, detour: Detour) -> Result<(), Detour> {
        self.detours.push(detour)
    }

    pub fn pop_detour(&mut self) -> Option<Detour> {
        self.detours.pop()
    }

    #[must_use]
    pub fn depth(&self) -> usize {
        self.detours.depth()
    }

    /// There is a plan or an open detour worth telling the talker about.
    #[must_use]
    pub fn has_context(&self) -> bool {
        !self.steps.is_empty() || self.detours.depth() > 0
    }

    /// The layer new drawing goes to: the innermost open detour, or the main board.
    #[must_use]
    pub fn active_layer(&self) -> LayerId {
        self.detours
            .top()
            .map_or(LayerId::MAIN, |detour| detour.layer)
    }

    #[must_use]
    pub fn detours(&self) -> &DetourStack {
        &self.detours
    }

    #[must_use]
    pub fn event(&self) -> UiEvent {
        UiEvent::Thread {
            steps: self.steps.clone(),
            current: self.current,
            detours: self
                .detours
                .items()
                .iter()
                .map(|detour| detour.title.clone())
                .collect(),
        }
    }

    /// Where the explanation is, for the talker: `step 2 of 3 "Edge"; in detour "cache-aside"`.
    #[must_use]
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        match (self.current, self.steps.is_empty()) {
            (_, true) => parts.push("no plan".to_owned()),
            (None, false) => parts.push(format!("plan {} (not started)", self.steps.join(" → "))),
            (Some(index), false) => {
                let name = self
                    .steps
                    .get(usize::from(index))
                    .map_or("", String::as_str);
                parts.push(format!(
                    "step {} of {} \"{name}\" in plan {}",
                    usize::from(index) + 1,
                    self.steps.len(),
                    self.steps.join(" → ")
                ));
            }
        }
        for detour in self.detours.items() {
            match &detour.anchor {
                None => parts.push(format!("in detour \"{}\"", detour.title)),
                Some(anchor) => parts.push(format!(
                    "in the detail of {} \"{}\"",
                    anchor.element.as_str(),
                    detour.title
                )),
            }
        }
        parts.join("; ")
    }
}

#[cfg(test)]
mod tests {
    use super::{Detour, Thread};
    use crate::ids::LayerId;
    use crate::limits::DETOUR_DEPTH_MAX;
    use crate::narration::ops::StepRef;

    #[test]
    fn steps_by_number_or_name() {
        let mut thread = Thread::default();
        thread.set_plan(vec!["Client".into(), "Edge".into(), "LB".into()]);
        assert!(thread.set_step(&StepRef::Number(2)));
        assert_eq!(
            thread.describe(),
            "step 2 of 3 \"Edge\" in plan Client → Edge → LB"
        );
        assert!(thread.set_step(&StepRef::Name("lb".into())));
        assert!(!thread.set_step(&StepRef::Number(4)));
        assert!(!thread.set_step(&StepRef::Number(0)));
    }

    /// I4 at the type level: the stack refuses a push past the limit.
    #[test]
    fn the_detour_stack_is_bounded() {
        let mut thread = Thread::default();
        for depth in 1..=DETOUR_DEPTH_MAX {
            let layer = LayerId(u8::try_from(depth).expect("small"));
            assert!(
                thread
                    .push_detour(Detour {
                        title: format!("d{depth}"),
                        layer,
                        anchor: None,
                    })
                    .is_ok()
            );
            assert_eq!(thread.active_layer(), layer);
        }
        let refused = thread.push_detour(Detour {
            title: "one too many".into(),
            layer: LayerId(9),
            anchor: None,
        });
        assert!(refused.is_err());
        assert_eq!(thread.depth(), DETOUR_DEPTH_MAX);
        while thread.pop_detour().is_some() {}
        assert_eq!(thread.active_layer(), LayerId::MAIN);
    }
}
