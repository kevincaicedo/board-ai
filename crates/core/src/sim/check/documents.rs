//! Law 6 for page chips (I17, D-257): a Doc card's chip names only what the session read of
//! that document: a page whose text a document read returned, or a page, a figure or a table its
//! digest holds. A citation the talker invented never gets a chip.

use std::collections::{BTreeMap, BTreeSet};

use crate::effect::Effect;
use crate::input::Input;
use crate::protocol::{Surface, UiEvent};
use crate::sources::digest::{Digest, parse};
use crate::sources::pages::find_cites;
use crate::sources::{SourceEffect, SourceId, SourceInput, name_key};
use crate::time::SessionTime;

#[derive(Clone, Debug, Default)]
pub struct ReadDocuments {
    /// Each document's names (its file name, its digest's title), by their letters and digits.
    names: BTreeMap<String, SourceId>,
    digests: BTreeMap<SourceId, Digest>,
    pages: BTreeSet<(SourceId, u32)>,
    /// Document reads asked of the runtime, by request.
    requests: BTreeMap<u32, SourceId>,
}

impl ReadDocuments {
    pub fn on_input(&mut self, input: &Input) {
        let Input::Source { source } = input else {
            return;
        };
        match source {
            SourceInput::Shared { source, .. } => {
                self.names.insert(name_key(&source.title), source.id);
            }
            SourceInput::Resolved { candidates, .. } => {
                for candidate in candidates {
                    self.names.insert(name_key(&candidate.title), candidate.id);
                }
            }
            SourceInput::ProjectLoaded { kept, .. } => {
                for kept in kept {
                    self.names
                        .insert(name_key(&kept.source.title), kept.source.id);
                }
            }
            SourceInput::DocumentRead {
                source,
                digest: Some(digest),
                ..
            } => {
                let digest = parse(digest);
                self.names.insert(name_key(&digest.title), *source);
                self.digests.insert(*source, digest);
            }
            SourceInput::DocumentPages { request, pages, .. } => {
                if let Some(source) = self.requests.get(request) {
                    for page in pages {
                        self.pages.insert((*source, page.page));
                    }
                }
            }
            SourceInput::Opened { .. }
            | SourceInput::BriefCached { .. }
            | SourceInput::BriefStarted { .. }
            | SourceInput::CitationsChecked { .. }
            | SourceInput::Dragging { .. }
            | SourceInput::Failed { .. }
            | SourceInput::RepoReads { .. }
            | SourceInput::DocumentRead { digest: None, .. } => {}
        }
    }

    pub fn on_effect(&mut self, effect: &Effect) {
        if let Effect::Source {
            source: SourceEffect::ReadDocument {
                request, source, ..
            },
        } = effect
        {
            self.requests.insert(*request, *source);
        }
    }

    fn page_read(&self, source: SourceId, page: u32) -> bool {
        self.pages.contains(&(source, page))
            || self
                .digests
                .get(&source)
                .is_some_and(|digest| digest.covers_page(page))
    }

    pub fn on_ui(&self, now: SessionTime, event: &UiEvent, violations: &mut Vec<String>) {
        let UiEvent::SurfaceShow {
            surface: Surface::Doc { title, chip, .. },
        } = event
        else {
            return;
        };
        let violation = |what: &str| {
            format!(
                "{} ms: I17: a Doc card shows \"{chip}\" of \"{title}\": {what}",
                now.as_ms()
            )
        };
        let Some(source) = self.names.get(&name_key(title)).copied() else {
            violations.push(violation("no document of that name was read"));
            return;
        };
        let digest = self.digests.get(&source);
        let cites = find_cites(chip);
        if cites.is_empty() {
            violations.push(violation("its chip cites nothing"));
        }
        for cite in cites {
            let figure = cite
                .figure
                .is_none_or(|number| digest.is_some_and(|digest| digest.figure(number).is_some()));
            let table = cite
                .table
                .is_none_or(|number| digest.is_some_and(|digest| digest.table(number).is_some()));
            let pages = cite.pages.is_none_or(|(first, last)| {
                (first..=last).all(|page| self.page_read(source, page))
            });
            if !(figure && table && pages) {
                violations.push(violation("it was never read"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ReadDocuments;
    use crate::effect::Effect;
    use crate::ids::SurfaceId;
    use crate::input::Input;
    use crate::protocol::{Surface, UiEvent};
    use crate::sources::{
        DocumentFormat, DocumentPage, SourceEffect, SourceId, SourceInfo, SourceInput, SourceKind,
        SourceState,
    };
    use crate::time::SessionTime;

    fn card(chip: &str) -> UiEvent {
        UiEvent::SurfaceShow {
            surface: Surface::Doc {
                id: SurfaceId(0),
                title: "Layered".into(),
                chip: chip.into(),
                figure: None,
                sentences: Vec::new(),
                highlight: None,
                note: String::new(),
            },
        }
    }

    /// The planted violations: a chip for a page or a figure nothing read turns the check red.
    #[test]
    fn a_chip_for_an_unread_page_or_figure_is_a_violation() {
        let mut read = ReadDocuments::default();
        let mut violations = Vec::new();
        read.on_ui(SessionTime::ZERO, &card("p. 3"), &mut violations);
        assert_eq!(violations.len(), 1, "no such document: {violations:?}");
        read.on_input(&Input::Source {
            source: SourceInput::Shared {
                source: SourceInfo {
                    id: SourceId(4),
                    kind: SourceKind::Document {
                        format: DocumentFormat::Pdf,
                    },
                    title: "layered.pdf".into(),
                    shown_path: "~/layered.pdf".into(),
                    files_estimate: None,
                },
                keep: false,
            },
        });
        read.on_input(&Input::Source {
            source: SourceInput::DocumentRead {
                source: SourceId(4),
                state: SourceState::Ready {
                    detail: String::new(),
                },
                digest: Some(
                    "# Digest: Layered\n## Figures\n### Figure 1 (p. 2): Parts.\nShows parts.\n"
                        .into(),
                ),
            },
        });
        violations.clear();
        read.on_ui(SessionTime::ZERO, &card("p. 2 · Figure 1"), &mut violations);
        assert!(violations.is_empty(), "{violations:?}");
        for unread in ["p. 3", "Figure 2", "p. 2 · Table 1", "pp. 2–3"] {
            read.on_ui(SessionTime::ZERO, &card(unread), &mut violations);
        }
        assert_eq!(violations.len(), 4, "{violations:?}");
        read.on_effect(&Effect::Source {
            source: SourceEffect::ReadDocument {
                request: 9,
                source: SourceId(4),
                first_page: 3,
                last_page: 3,
            },
        });
        read.on_input(&Input::Source {
            source: SourceInput::DocumentPages {
                request: 9,
                pages: vec![DocumentPage {
                    page: 3,
                    text: "Text.".into(),
                }],
                page_count: Some(7),
            },
        });
        violations.clear();
        read.on_ui(SessionTime::ZERO, &card("pp. 2–3"), &mut violations);
        assert!(violations.is_empty(), "{violations:?}");
    }
}
