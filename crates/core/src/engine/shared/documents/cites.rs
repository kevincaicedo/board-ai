//! Page citations and the Doc card (D-257, I17). A citation in a talker clause ("Figure 1
//! (p. 3)", "page 3", "Table 2") waits until the voice reaches it, then is checked against the
//! document's digest and the pages the session read (law 6). One that checks out opens the Doc
//! card on that passage or figure, with its chip; one that doesn't shows nothing and the talker
//! is told. While a Doc card is open, each clause that starts moves its highlight to the
//! sentence it explains.

use crate::effect::Effect;
use crate::engine::Core;
use crate::ids::ClauseId;
use crate::limits::{
    DOC_CARD_SENTENCES_MAX, DOC_CARD_TITLE_MAX_BYTES, DOCUMENT_CUES_WAITING_MAX,
    DOCUMENT_PAGES_KEPT_MAX, PAGE_CITES_PER_REPLY_MAX,
};
use crate::narration::tags::bounded;
use crate::protocol::DocFigure;
use crate::sources::SourceId;
use crate::sources::digest::{Digest, Locator};
use crate::sources::pages::{PageCite, best_sentence, find_cites, passage, sentences};
use crate::surfaces::DocCard;

/// Something in the talker's speech waiting for the voice.
#[derive(Clone, Debug)]
pub(in crate::engine) enum Cue {
    /// A citation, due at its first character.
    Cite {
        clause: ClauseId,
        offset: usize,
        cite: PageCite,
        said: String,
    },
    /// A clause starting: the Doc card's highlight follows it.
    Clause { clause: ClauseId, said: String },
}

impl Cue {
    const fn anchor(&self) -> (ClauseId, usize) {
        match self {
            Self::Cite { clause, offset, .. } => (*clause, *offset),
            Self::Clause { clause, .. } => (*clause, 0),
        }
    }
}

/// A citation that checked out: the card's content.
struct Checked {
    source: SourceId,
    card: DocCard,
}

impl Core {
    /// A talker clause was sent to speech: its start and its citations wait for the voice.
    pub(in crate::engine) fn follow_clause(&mut self, clause: ClauseId) {
        if !self.sources.documents.is_active() {
            return;
        }
        let Some(spoken) = self.voice.clause(clause) else {
            return;
        };
        let said = spoken.caption.clone();
        let mut cues = vec![Cue::Clause {
            clause,
            said: said.clone(),
        }];
        for cite in find_cites(&said) {
            cues.push(Cue::Cite {
                clause,
                offset: spoken.speakable.speakable_offset(cite.at),
                cite,
                said: said.clone(),
            });
        }
        let waiting = &mut self.sources.documents.cues;
        for cue in cues {
            waiting.push_back(cue);
            while waiting.len() > DOCUMENT_CUES_WAITING_MAX {
                waiting.pop_front();
            }
        }
    }

    /// Fires every cue whose words have played, in order; a cue in a clause that will never
    /// play is dropped (an interruption commits only what was heard, and shows nothing more).
    pub(in crate::engine) fn advance_document_cues(&mut self, effects: &mut Vec<Effect>) {
        if self.stop.is_some() {
            return;
        }
        // Each pass fires or drops a cue, or stops; the queue's length bounds the loop.
        for _ in 0..=self.sources.documents.cues.len() {
            let Some(cue) = self.sources.documents.cues.front() else {
                return;
            };
            let (clause, offset) = cue.anchor();
            let never = self.voice.clause(clause).is_none() || self.voice.skipped(clause);
            if !never && !self.voice.reached(clause, offset) {
                return;
            }
            let Some(cue) = self.sources.documents.cues.pop_front() else {
                return;
            };
            if never || self.has_ended() {
                continue;
            }
            match cue {
                Cue::Cite { cite, said, .. } => self.on_page_cite(cite, &said, effects),
                Cue::Clause { said, .. } => self.follow_sentence(&said, effects),
            }
        }
    }

    fn follow_sentence(&mut self, said: &str, effects: &mut Vec<Effect>) {
        let Some((_, sentences, _)) = self.surfaces.doc_card() else {
            return;
        };
        let Some(index) = best_sentence(sentences, said) else {
            return;
        };
        let mut events = Vec::new();
        let index = u32::try_from(index).unwrap_or(u32::MAX);
        if self.surfaces.highlight_sentence(index, &mut events) {
            effects.extend(events.into_iter().map(Effect::Ui));
        }
    }

    /// A citation the voice reached: the Doc card when it checks out, else nothing shown and
    /// the talker told (law 6).
    fn on_page_cite(&mut self, cite: PageCite, said: &str, effects: &mut Vec<Effect>) {
        let checked = self.cited_documents().into_iter().find_map(|source| {
            let card = self.check_cite(source, cite, said)?;
            Some(Checked { source, card })
        });
        let Some(Checked { source, card }) = checked else {
            self.cite_missed(cite);
            return;
        };
        self.sources.documents.last = Some(source);
        let key = (format!("doc:{}:{}", source.0, card.chip), 0);
        let Some(reply) = self.reply.as_mut() else {
            return;
        };
        let shown = reply
            .cited
            .iter()
            .filter(|(key, _)| key.starts_with("doc:"))
            .count();
        if reply.cited.contains(&key) || shown >= PAGE_CITES_PER_REPLY_MAX {
            // The same passage again: its highlight follows the clause instead.
            self.follow_sentence(said, effects);
            return;
        }
        reply.cited.insert(key);
        let mut events = Vec::new();
        self.surfaces.open_doc(card, &mut events);
        effects.extend(events.into_iter().map(Effect::Ui));
    }

    /// The documents a bare citation may mean: the one last read or cited first, then the rest,
    /// newest first.
    fn cited_documents(&self) -> Vec<SourceId> {
        let desk = &self.sources.documents;
        let mut order: Vec<SourceId> = desk.last.into_iter().collect();
        for entry in self.sources.shared.entries().iter().rev() {
            let id = entry.info.id;
            let known =
                desk.digest(id).is_some() || desk.pages.iter().any(|kept| kept.source == id);
            if known && !order.contains(&id) {
                order.push(id);
            }
        }
        order
            .into_iter()
            .filter(|id| self.sources.shared.get(*id).is_some())
            .collect()
    }

    /// Whether page `page` of `source` was read: its text this session, or its digest.
    fn page_known(&self, source: SourceId, page: u32) -> bool {
        let desk = &self.sources.documents;
        desk.page_text(source, page).is_some()
            || desk
                .digest(source)
                .is_some_and(|digest| digest.covers_page(page))
    }

    /// The card a citation opens on `source`, when everything it names was read there.
    fn check_cite(&self, source: SourceId, cite: PageCite, said: &str) -> Option<DocCard> {
        let digest = self.sources.documents.digest(source);
        let title = bounded(&self.document_title(source), DOC_CARD_TITLE_MAX_BYTES);
        // A range wider than the pages a session keeps can't all have been read.
        let span_max = u32::try_from(DOCUMENT_PAGES_KEPT_MAX).unwrap_or(u32::MAX);
        let pages_known = |(first, last): (u32, u32)| {
            last - first < span_max && (first..=last).all(|page| self.page_known(source, page))
        };
        if let Some(number) = cite.figure.or(cite.table) {
            let digest = digest?;
            let is_figure = cite.figure.is_some();
            let (at, card) = if is_figure {
                let figure = digest.figure(number)?;
                (figure.at, figure_card(&title, digest, figure, said))
            } else {
                let table = digest.table(number)?;
                (table.at, table_card(&title, digest, table, said))
            };
            if let Some(pages) = cite.pages {
                let placed = at.is_some_and(|at| at.covers_page(pages.0));
                if !placed && !pages_known(pages) {
                    return None;
                }
            }
            let page = cite
                .pages
                .map(|(first, _)| first)
                .or_else(|| at?.first_page());
            let label = if is_figure { "Figure" } else { "Table" };
            let chip = match page {
                Some(page) => format!("p. {page} · {label} {number}"),
                None => format!("{label} {number}"),
            };
            return Some(DocCard { chip, ..card });
        }
        let pages = cite.pages?;
        if !pages_known(pages) {
            return None;
        }
        Some(self.page_card(source, &title, pages, said))
    }

    fn page_card(&self, source: SourceId, title: &str, pages: (u32, u32), said: &str) -> DocCard {
        let (first, last) = pages;
        let chip = Locator::Pages { first, last }.label();
        let (sentences, highlight, note) = match self.sources.documents.page_text(source, first) {
            Some(text) => {
                let (sentences, at) = passage(text, said, DOC_CARD_SENTENCES_MAX);
                let note = format!("Page {first} as BoardAI extracted it on this Mac.");
                (sentences, at, note)
            }
            None => {
                let text = self
                    .sources
                    .documents
                    .digest(source)
                    .map(|digest| digest_on_page(digest, first))
                    .unwrap_or_default();
                let (sentences, at) = passage(&text, said, DOC_CARD_SENTENCES_MAX);
                (sentences, at, format!("From the digest of page {first}."))
            }
        };
        DocCard {
            title: title.to_owned(),
            chip,
            figure: None,
            sentences,
            highlight: highlight.and_then(|index| u32::try_from(index).ok()),
            note,
        }
    }

    /// A citation that didn't check out: no chip, and the talker is told once a reply.
    fn cite_missed(&mut self, cite: PageCite) {
        let Some(source) = self.cited_documents().into_iter().next() else {
            return;
        };
        let what = match (cite.figure, cite.table, cite.pages) {
            (Some(figure), _, _) => format!("Figure {figure}"),
            (None, Some(table), _) => format!("Table {table}"),
            (None, None, Some((first, last))) => Locator::Pages { first, last }.label(),
            (None, None, None) => return,
        };
        let key = (format!("miss:{}:{what}", source.0), 0);
        let Some(reply) = self.reply.as_mut() else {
            return;
        };
        if !reply.cited.insert(key) {
            return;
        }
        // The file's name: the digest's title is the document's words (D-265).
        let title = self.file_name(source);
        self.context.notice(format!(
            "You cited {what} of {title}, but it isn't in its digest and wasn't read in this \
             session, so no page chip was shown. Read it with read_file before citing it."
        ));
    }
}

fn figure_card(
    title: &str,
    digest: &Digest,
    figure: &crate::sources::digest::DigestFigure,
    said: &str,
) -> DocCard {
    let read = figure.is_read();
    let found = if read {
        sentences(&figure.shows, DOC_CARD_SENTENCES_MAX)
    } else {
        Vec::new()
    };
    let note = match (&digest.read.figures_by, read, figure.diagram.is_some()) {
        (Some(by), true, true) => {
            format!("Figure read by {by}. Redrawn on the board from the digest.")
        }
        (Some(by), true, false) => format!("Figure read by {by}."),
        (None, true, _) => "Figure read with the document's digest.".to_owned(),
        (Some(_) | None, false, _) => {
            "Only its caption was read: the figure needs a model that reads files.".to_owned()
        }
    };
    let highlight = best_sentence(&found, said).and_then(|index| u32::try_from(index).ok());
    DocCard {
        title: title.to_owned(),
        chip: String::new(),
        figure: Some(DocFigure {
            label: format!("Figure {}", figure.number),
            caption: figure.caption.clone(),
            read,
        }),
        sentences: found,
        highlight,
        note,
    }
}

fn table_card(
    title: &str,
    digest: &Digest,
    table: &crate::sources::digest::DigestTable,
    said: &str,
) -> DocCard {
    let rows: Vec<String> = table
        .rows
        .iter()
        .filter(|row| !row.chars().all(|ch| matches!(ch, '|' | '-' | ':' | ' ')))
        .take(DOC_CARD_SENTENCES_MAX)
        .cloned()
        .collect();
    let note = match &digest.read.figures_by {
        Some(by) => format!("Table read by {by}."),
        None => "Table as BoardAI extracted it on this Mac.".to_owned(),
    };
    let highlight = best_sentence(&rows, said).and_then(|index| u32::try_from(index).ok());
    DocCard {
        title: title.to_owned(),
        chip: String::new(),
        figure: Some(DocFigure {
            label: format!("Table {}", table.number),
            caption: table.caption.clone(),
            read: true,
        }),
        sentences: rows,
        highlight,
        note,
    }
}

/// What the digest says about page `page`: the sections over it, then its claims and the
/// captions of its figures and tables.
fn digest_on_page(digest: &Digest, page: u32) -> String {
    let on = |at: &Option<Locator>| at.is_some_and(|at| at.covers_page(page));
    let mut text = Vec::new();
    for section in digest.sections.iter().filter(|section| on(&section.at)) {
        text.push(section.summary.clone());
    }
    for claim in digest.claims.iter().filter(|claim| on(&claim.at)) {
        text.push(claim.text.clone());
    }
    for figure in digest.figures.iter().filter(|figure| on(&figure.at)) {
        text.push(format!("Figure {}: {}", figure.number, figure.caption));
    }
    let ended: Vec<String> = text
        .into_iter()
        .filter(|part| !part.is_empty())
        .map(|part| {
            if part.ends_with(['.', '?', '!']) {
                part
            } else {
                format!("{part}.")
            }
        })
        .collect();
    ended.join(" ")
}
