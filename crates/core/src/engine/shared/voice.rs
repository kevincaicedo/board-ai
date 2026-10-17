//! Sources by voice (PRODUCT §7.7): picking one of several matches, the tray, the project,
//! keeping, sharing by name, and "more detail on X". Each is checked at the end of the user's
//! turn, after the memory controls, in that order (`sources::cues`).

use crate::effect::Effect;
use crate::engine::Core;
use crate::engine::shared::Resolving;
use crate::engine::shared::briefs::Waiting;
use crate::limits::SOURCE_RESOLVES_WAITING_MAX;
use crate::protocol::UiEvent;
use crate::sources::cues::{self, KeepCue, ShareCue, TrayCue};
use crate::sources::{BriefTopic, Project, SourceEffect, SourceId};
use crate::time::SessionTime;

impl Core {
    /// Words about sources. Returns true when the core answered them itself (the talker hears
    /// of it in a notice); false when the words go on to the talker as usual.
    pub(in crate::engine) fn sources_by_voice(
        &mut self,
        now: SessionTime,
        text: &str,
        effects: &mut Vec<Effect>,
    ) -> bool {
        if self.has_ended() {
            return false;
        }
        if self.resolve_choice(now, text, effects) {
            return true;
        }
        if let Some(cue) = cues::tray_cue(text) {
            self.tray_by_voice(cue, effects);
            return true;
        }
        if let Some((name, rest)) = cues::project_cue(text) {
            return self.project_by_voice(now, &name, rest.is_none(), effects);
        }
        if self.keep_control(now, text, effects) {
            return true;
        }
        if let Some(cue) = cues::share_cue(text) {
            self.resolve_by_voice(cue, effects);
            return true;
        }
        self.source_detail_by_voice(text, effects);
        false
    }

    /// "Show sources", "hide sources": alone, or joined to other controls (`joined_cues`).
    pub(in crate::engine) fn tray_by_voice(&mut self, cue: TrayCue, effects: &mut Vec<Effect>) {
        if self.has_ended() {
            return;
        }
        let _ = self.sources.shared.set_open(cue == TrayCue::Show);
        effects.push(Effect::Ui(UiEvent::Acknowledge));
        self.show_tray(effects);
    }

    /// "Keep this in the project": alone, or joined to other controls. True when the words
    /// were a keep (the core answered them).
    pub(in crate::engine) fn keep_control(
        &mut self,
        now: SessionTime,
        text: &str,
        effects: &mut Vec<Effect>,
    ) -> bool {
        if self.has_ended() {
            return false;
        }
        let Some(cue) = cues::keep_cue(text) else {
            return false;
        };
        self.keep_by_voice(now, cue, effects);
        true
    }

    /// An open choice is answered by the next words: they pick one, or the choice closes and
    /// the words are heard as usual. The candidates not picked are discarded, except one this
    /// session already shares or keeps (the runtime names a path by the id it has).
    fn resolve_choice(&mut self, now: SessionTime, text: &str, effects: &mut Vec<Effect>) -> bool {
        let Some(choice) = self.sources.choice.take() else {
            return false;
        };
        let paths: Vec<&str> = choice
            .candidates
            .iter()
            .map(|candidate| candidate.shown_path.as_str())
            .collect();
        let picked = cues::choice(text, &paths);
        let mut others: Vec<SourceId> = Vec::new();
        let mut chosen = None;
        for (index, candidate) in choice.candidates.into_iter().enumerate() {
            if Some(index) == picked {
                chosen = Some(candidate);
            } else {
                others.push(candidate.id);
            }
        }
        self.discard_unheld(others, effects);
        let Some(chosen) = chosen else {
            return false;
        };
        self.share_candidate(now, chosen, &choice.keep, effects);
        self.show_tray(effects);
        true
    }

    fn project_by_voice(
        &mut self,
        now: SessionTime,
        name: &str,
        alone: bool,
        effects: &mut Vec<Effect>,
    ) -> bool {
        let Some(project) = Project::named(name) else {
            return false;
        };
        let spoken = project.name.clone();
        self.use_project(now, project, false, effects);
        self.show_tray(effects);
        if alone {
            self.say(&format!("Working on {spoken}."), effects);
        }
        self.context.notice(format!(
            "The session works on the {spoken} project now; its kept sources are loading."
        ));
        alone
    }

    fn keep_by_voice(&mut self, now: SessionTime, cue: KeepCue, effects: &mut Vec<Effect>) {
        let target = match &cue.source {
            Some(name) => self.sources.shared.named(name),
            None => self.sources.shared.newest_in_session(),
        };
        match (target, cue.source) {
            (Some(id), _) => {
                if let Some(project) = self.keep_source(now, id, cue.project, effects) {
                    self.say(&format!("Kept in {project}."), effects);
                }
            }
            // Not shared yet: find it first, then keep it.
            (None, Some(name)) => self.resolve_by_voice(
                ShareCue {
                    name,
                    kind: crate::sources::NameKind::Folder,
                    keep: cue.project,
                },
                effects,
            ),
            (None, None) => self.say("There's nothing shared to keep yet.", effects),
        }
        self.show_tray(effects);
    }

    /// Asks the runtime to find a spoken name; the answer is `Resolved`.
    fn resolve_by_voice(&mut self, cue: ShareCue, effects: &mut Vec<Effect>) {
        let request = self.sources.next_request;
        self.sources.next_request = request.wrapping_add(1);
        while self.sources.resolving.len() >= SOURCE_RESOLVES_WAITING_MAX {
            // The runtime answers every request; one this old was lost with a closing task.
            let _ = self.sources.resolving.pop_first();
        }
        self.sources.resolving.insert(
            request,
            Resolving {
                name: cue.name.clone(),
                keep: cue.keep,
            },
        );
        effects.push(Effect::Source {
            source: SourceEffect::Resolve {
                request,
                name: cue.name,
                kind: cue.kind,
            },
        });
    }

    /// "More detail on the arena": when a shared code source has that part, its component
    /// brief is asked for at once (D-188), and the talker hears the words with a note. A detail
    /// layer on the board's part of that name opens as well (`engine::detail`); the brief fills
    /// it once it is here.
    pub(in crate::engine) fn source_detail_by_voice(
        &mut self,
        text: &str,
        effects: &mut Vec<Effect>,
    ) {
        let Some(name) = cues::detail_cue(text) else {
            return;
        };
        let Some((source, part)) = self.sources.shared.with_component(&name) else {
            return;
        };
        let Some(entry) = self.sources.shared.get(source) else {
            return;
        };
        // The part is named by its id in the notes: its name is the brief's words (D-265).
        let title = entry.info.title.clone();
        let part_name = entry
            .component(&part)
            .map_or(part.clone(), |(_, label)| label.clone());
        let topic = BriefTopic::component(&part_name);
        let harness = self.sources.shared.harness_name().to_owned();
        if entry.delivered.contains(&topic) {
            self.context.notice(format!(
                "{harness}'s brief on part {part} of {title} is already in this conversation; \
                 answer from it."
            ));
            return;
        }
        if let Some(task) = self.brief_running(source, &topic) {
            // A call held for it keeps its place: it gets the brief (D-268).
            if let Some(running) = self.sources.briefs.get_mut(&task)
                && running.waiting == Waiting::Nobody
            {
                running.waiting = Waiting::Talker;
            }
        } else if self
            .request_brief(source, topic, Some(text), Waiting::Talker, effects)
            .is_none()
        {
            return;
        }
        self.context.notice(format!(
            "{harness} is reading part {part} of {title} in more detail; its brief arrives soon, \
             between task-output marks. Say so in a few words, and explain from the brief once \
             it is here."
        ));
    }
}
