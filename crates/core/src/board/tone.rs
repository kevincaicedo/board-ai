//! Hachure tones for nodes (the canvas's sketch fills). The core picks them so a board looks the
//! same live, in replay and in the export, and the UI only draws what it is told (law 1; this
//! replaces the UI's interim heuristic, D-031).
//!
//! The rule reproduces the canvas demo: a store is sky and a queue is mint; each new service
//! subject takes the next tone of sky, lilac, mint, blush on its layer, and a subject keeps its
//! tone everywhere (the detour's Redis is the main board's Redis). A detour starts two tones
//! along, so its card doesn't echo the board behind it. The talker may name a tone (`fill="mint"`),
//! and the subject keeps that one too. Actors, groups and notes have no fill.

use std::collections::BTreeMap;

use crate::ids::LayerId;
use crate::protocol::{Fill, NodeShape};

/// The order new service subjects take tones in.
const CYCLE: [Fill; 4] = [Fill::Sky, Fill::Lilac, Fill::Mint, Fill::Blush];

/// How far along the cycle each detour layer starts, per level of layer id.
const DETOUR_OFFSET: usize = 2;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Tones {
    /// A subject's tone, by its label trimmed and lowercased. Bounded by the elements on the
    /// board: an entry is added only when a node is laid out.
    by_subject: BTreeMap<String, Fill>,
    /// How many new service subjects each layer has seen.
    new_subjects: BTreeMap<LayerId, usize>,
}

impl Tones {
    /// The tone for a node being laid out; `chosen` is the talker's `fill`, if any.
    pub(super) fn pick(
        &mut self,
        layer: LayerId,
        shape: NodeShape,
        label: &str,
        chosen: Option<Fill>,
    ) -> Option<Fill> {
        let subject = label.trim().to_lowercase();
        match shape {
            NodeShape::Actor | NodeShape::Group | NodeShape::Note => return None,
            NodeShape::Db | NodeShape::Queue | NodeShape::Service => {}
        }
        if let Some(chosen) = chosen {
            self.by_subject.insert(subject, chosen);
            return Some(chosen);
        }
        match shape {
            NodeShape::Db => return Some(Fill::Sky),
            NodeShape::Queue => return Some(Fill::Mint),
            NodeShape::Service | NodeShape::Actor | NodeShape::Group | NodeShape::Note => {}
        }
        if let Some(known) = self.by_subject.get(&subject) {
            return Some(*known);
        }
        let count = self.new_subjects.entry(layer).or_default();
        let tone = CYCLE[(usize::from(layer.0) * DETOUR_OFFSET + *count) % CYCLE.len()];
        *count += 1;
        self.by_subject.insert(subject, tone);
        Some(tone)
    }
}

#[cfg(test)]
mod tests {
    use super::Tones;
    use crate::ids::LayerId;
    use crate::protocol::{Fill, NodeShape};

    /// The canvas demo's colors: CDN sky, load balancer lilac, the pods mint, Redis blush,
    /// Postgres sky; in the detour, API takes mint (two along) and Redis stays blush.
    #[test]
    fn reproduces_the_canvas_demo() {
        let mut tones = Tones::default();
        let main = LayerId::MAIN;
        let detour = LayerId(1);
        let picks = [
            (main, NodeShape::Actor, "Client"),
            (main, NodeShape::Service, "CDN"),
            (main, NodeShape::Service, "Load balancer"),
            (main, NodeShape::Group, "API pods ×3"),
            (main, NodeShape::Service, ""),
            (main, NodeShape::Service, ""),
            (main, NodeShape::Service, "Redis"),
            (main, NodeShape::Db, "Postgres"),
            (detour, NodeShape::Service, "API"),
            (detour, NodeShape::Service, "redis "),
        ]
        .map(|(layer, shape, label)| tones.pick(layer, shape, label, None));
        assert_eq!(
            picks,
            [
                None,
                Some(Fill::Sky),
                Some(Fill::Lilac),
                None,
                Some(Fill::Mint),
                Some(Fill::Mint),
                Some(Fill::Blush),
                Some(Fill::Sky),
                Some(Fill::Mint),
                Some(Fill::Blush),
            ]
        );
    }

    #[test]
    fn the_talker_may_choose_and_the_subject_keeps_it() {
        let mut tones = Tones::default();
        let main = LayerId::MAIN;
        assert_eq!(
            tones.pick(main, NodeShape::Service, "Kafka", Some(Fill::Blush)),
            Some(Fill::Blush)
        );
        assert_eq!(
            tones.pick(main, NodeShape::Service, "kafka", None),
            Some(Fill::Blush)
        );
        assert_eq!(
            tones.pick(main, NodeShape::Db, "Orders", Some(Fill::Lilac)),
            Some(Fill::Lilac)
        );
        assert_eq!(
            tones.pick(main, NodeShape::Actor, "User", Some(Fill::Mint)),
            None,
            "a stick figure takes no fill"
        );
        assert_eq!(
            tones.pick(main, NodeShape::Service, "API", None),
            Some(Fill::Sky),
            "chosen tones don't use up the cycle"
        );
    }
}
