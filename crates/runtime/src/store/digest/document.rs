//! `digest.md`: rendered by the store, parsed back by the index. The sections are stable so a
//! rebuild recovers the graph and the proposals; a person may edit any of it.

use super::{About, Digest, Entity, EntityKind, Relation, RelationKind, WrittenBy};
use crate::limits::VAULT_ITEM_MAX_BYTES;
use crate::store::ids::{ItemId, SessionId};
use crate::store::time::WallTime;
use crate::store::vault::BOARD_FILE;
use crate::store::vault::markdown::{
    FrontValue, one_line, parse_item, render_front_matter, render_item, split_front_matter,
};

const SUMMARY: &str = "Summary";
const DIAGRAMS: &str = "Key diagrams";
const QUESTIONS: &str = "Open questions";
const CAPTURED: &str = "Captured";
const PROPOSALS: &str = "Proposed memories";
const GRAPH: &str = "Graph";
const ABOUT_YOU_PREFIX: &str = "About you: ";
const ABOUT_SYSTEMS_PREFIX: &str = "About your systems: ";

/// A proposal as listed in the digest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProposalLine {
    pub id: ItemId,
    pub about: About,
    pub text: String,
    pub accepted: bool,
}

/// Everything `digest.md` is rendered from.
pub(crate) struct DigestDocument<'a> {
    pub id: &'a SessionId,
    pub started: WallTime,
    pub duration_ms: u64,
    pub digest: &'a Digest,
    pub has_board: bool,
    /// One line per item captured in the session, already rendered.
    pub captured: &'a [String],
    pub proposals: &'a [ProposalLine],
}

impl DigestDocument<'_> {
    /// The file's text, before redaction (the caller redacts it on the way to disk).
    pub(crate) fn render(&self) -> String {
        let digest = self.digest;
        let written_by = match &digest.written_by {
            WrittenBy::Scribe => "scribe",
            WrittenBy::Fallback { .. } => "fallback",
        };
        let mut out = render_front_matter(&[
            ("id", FrontValue::Text(self.id.to_string())),
            ("title", FrontValue::Text(digest.title.clone())),
            ("started", FrontValue::Text(self.started.rfc3339())),
            (
                "duration_ms",
                FrontValue::Text(self.duration_ms.to_string()),
            ),
            ("written_by", FrontValue::Text(written_by.to_owned())),
        ]);
        out.push_str(&format!(
            "# {}\n\n## {SUMMARY}\n\n{}\n",
            digest.title, digest.summary
        ));
        let diagrams: Vec<String> = match (self.has_board, digest.diagrams.is_empty()) {
            (true, true) => vec![format!("[Board]({BOARD_FILE})")],
            (true, false) => digest
                .diagrams
                .iter()
                .map(|name| format!("[{name}]({BOARD_FILE})"))
                .collect(),
            (false, _) => digest.diagrams.clone(),
        };
        push_list(&mut out, DIAGRAMS, &diagrams);
        push_list(&mut out, QUESTIONS, &digest.open_questions);
        push_list(&mut out, CAPTURED, self.captured);
        if !self.proposals.is_empty() {
            out.push_str(&format!(
                "\n## {PROPOSALS}\n\nNothing here is kept until you say yes.\n\n"
            ));
            for proposal in self.proposals {
                let prefix = match proposal.about {
                    About::You => ABOUT_YOU_PREFIX,
                    About::Systems => ABOUT_SYSTEMS_PREFIX,
                };
                let text = format!("{prefix}{}", proposal.text);
                out.push_str(&render_item(
                    Some(proposal.accepted),
                    &text,
                    &[],
                    Some(proposal.id),
                ));
                out.push('\n');
            }
        }
        let mut graph: Vec<String> = digest
            .entities
            .iter()
            .map(|entity| format!("{} ({})", entity.name, entity.kind.as_str()))
            .collect();
        graph.extend(digest.relations.iter().map(|relation| {
            format!(
                "{} `{}` {}",
                relation.subject,
                relation.relation.as_str(),
                relation.object
            )
        }));
        push_list(&mut out, GRAPH, &graph);
        out
    }
}

fn push_list(out: &mut String, heading: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    out.push_str(&format!("\n## {heading}\n\n"));
    for item in items {
        out.push_str("- ");
        out.push_str(&one_line(item, VAULT_ITEM_MAX_BYTES));
        out.push('\n');
    }
}

/// What the index reads back from a digest. Every field is optional or may be empty: the file
/// may have been edited by hand.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ParsedDigest {
    pub id: Option<String>,
    pub title: Option<String>,
    pub started: Option<WallTime>,
    pub duration_ms: Option<u64>,
    pub summary: String,
    pub open_questions: Vec<String>,
    pub proposals: Vec<ProposalLine>,
    pub entities: Vec<Entity>,
    pub relations: Vec<Relation>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Other,
    Summary,
    Questions,
    Proposals,
    Graph,
}

pub(crate) fn parse_digest(text: &str) -> ParsedDigest {
    let (front, body) = split_front_matter(text);
    let front = front.unwrap_or_default();
    let mut parsed = ParsedDigest {
        id: front.text("id").map(str::to_owned),
        title: front.text("title").map(str::to_owned),
        started: front.text("started").and_then(WallTime::parse_rfc3339),
        duration_ms: front.text("duration_ms").and_then(|ms| ms.parse().ok()),
        ..ParsedDigest::default()
    };
    let mut section = Section::Other;
    let mut summary_lines: Vec<&str> = Vec::new();
    for line in body.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            section = match heading.trim() {
                SUMMARY => Section::Summary,
                QUESTIONS => Section::Questions,
                PROPOSALS => Section::Proposals,
                GRAPH => Section::Graph,
                _ => Section::Other,
            };
            continue;
        }
        if let Some(title) = line.strip_prefix("# ") {
            parsed.title.get_or_insert_with(|| title.trim().to_owned());
            continue;
        }
        match section {
            Section::Summary if !line.trim().is_empty() => summary_lines.push(line.trim()),
            Section::Questions => {
                if let Some(item) = parse_item(line) {
                    parsed.open_questions.push(item.text.to_owned());
                }
            }
            Section::Proposals => parsed.proposals.extend(parse_proposal(line)),
            Section::Graph => parse_graph_line(line, &mut parsed),
            Section::Summary | Section::Other => {}
        }
    }
    parsed.summary = summary_lines.join(" ");
    parsed
}

fn parse_proposal(line: &str) -> Option<ProposalLine> {
    let item = parse_item(line)?;
    let (about, text) = if let Some(text) = item.text.strip_prefix(ABOUT_YOU_PREFIX) {
        (About::You, text)
    } else if let Some(text) = item.text.strip_prefix(ABOUT_SYSTEMS_PREFIX) {
        (About::Systems, text)
    } else {
        (About::Systems, item.text)
    };
    Some(ProposalLine {
        id: item.id?,
        about,
        text: text.to_owned(),
        accepted: item.checked.unwrap_or(false),
    })
}

fn parse_graph_line(line: &str, parsed: &mut ParsedDigest) {
    let Some(item) = parse_item(line) else {
        return;
    };
    let text = item.text;
    let mut parts = text.splitn(3, '`');
    if let (Some(subject), Some(relation), Some(object)) =
        (parts.next(), parts.next(), parts.next())
    {
        if let Some(relation) = RelationKind::parse(relation.trim()) {
            let (subject, object) = (subject.trim(), object.trim());
            if !subject.is_empty() && !object.is_empty() {
                parsed.relations.push(Relation {
                    subject: subject.to_owned(),
                    relation,
                    object: object.to_owned(),
                });
            }
        }
        return;
    }
    let entity = text
        .strip_suffix(')')
        .and_then(|rest| rest.rsplit_once(" ("))
        .and_then(|(name, kind)| Some((name.trim(), EntityKind::parse(kind)?)));
    let (name, kind) = entity.unwrap_or((text.trim(), EntityKind::Concept));
    if !name.is_empty() {
        parsed.entities.push(Entity {
            name: name.to_owned(),
            kind,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{DigestDocument, ProposalLine, parse_digest};
    use crate::store::digest::{
        About, Digest, Entity, EntityKind, FallbackReason, Relation, RelationKind, WrittenBy,
    };
    use crate::store::ids::{ItemId, ItemKind, SessionId};
    use crate::store::time::WallTime;

    fn digest() -> Digest {
        Digest {
            title: "How a request reaches the API".into(),
            summary: "We traced a request from the client to the API.".into(),
            diagrams: vec!["Request path".into()],
            open_questions: vec!["Why does the CDN cache 404s?".into()],
            proposals: Vec::new(),
            entities: vec![Entity {
                name: "Redis (cluster)".into(),
                kind: EntityKind::System,
            }],
            relations: vec![Relation {
                subject: "API".into(),
                relation: RelationKind::DependsOn,
                object: "Redis (cluster)".into(),
            }],
            written_by: WrittenBy::Fallback {
                reason: FallbackReason::NoScribe,
            },
        }
    }

    #[test]
    fn rendered_digests_parse_back() {
        let id = SessionId::parse("2026-10-15-143210").unwrap_or_else(|| unreachable!());
        let digest = digest();
        let proposals = [ProposalLine {
            id: ItemId::new(ItemKind::Proposal, 3),
            about: About::You,
            text: "Likes a slower pace".into(),
            accepted: false,
        }];
        let text = DigestDocument {
            id: &id,
            started: WallTime::from_unix_ms(1_792_074_730_000),
            duration_ms: 61_000,
            digest: &digest,
            has_board: true,
            captured: &["Task: Check Redis TTLs (projects/inbox.md ^t27)".into()],
            proposals: &proposals,
        }
        .render();
        assert!(
            text.contains("- [Request path](board.excalidraw)"),
            "{text}"
        );
        let parsed = parse_digest(&text);
        assert_eq!(parsed.id.as_deref(), Some("2026-10-15-143210"));
        assert_eq!(parsed.title.as_deref(), Some(digest.title.as_str()));
        assert_eq!(
            parsed.started,
            Some(WallTime::from_unix_ms(1_792_074_730_000))
        );
        assert_eq!(parsed.duration_ms, Some(61_000));
        assert_eq!(parsed.summary, digest.summary);
        assert_eq!(parsed.open_questions, digest.open_questions);
        assert_eq!(parsed.proposals, proposals);
        assert_eq!(parsed.entities, digest.entities);
        assert_eq!(parsed.relations, digest.relations);
    }

    #[test]
    fn hand_edited_digests_parse_leniently() {
        let parsed = parse_digest("# My title\n\n## Graph\n\n- Kafka\n- a `nonsense` b\n");
        assert_eq!(parsed.title.as_deref(), Some("My title"));
        assert_eq!(parsed.entities.len(), 1);
        assert!(parsed.relations.is_empty());
    }
}
