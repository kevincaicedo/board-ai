//! Reads over the index, on the read-only connection: recall, the library, the Now list, a
//! session lookup and the "where we left off" recap. Every answer is bounded.

use rusqlite::{Connection, OptionalExtension, params};

use crate::limits::{
    LIBRARY_ENTRIES_MAX, NOW_LIST_MAX, RECALL_HITS_MAX, RECALL_QUERY_TERMS_MAX,
    RECALL_TEXT_MAX_BYTES, RECAP_MAX_CHARS,
};
use crate::store::error::StoreError;
use crate::store::ids::SessionId;
use crate::store::slug::slugify;
use crate::store::time::{WallTime, date_of};
use crate::store::vault::markdown::cut_chars;
use crate::store::vault::{BOARD_FILE, VaultPath};
use crate::store::views::{Library, LibraryEntry, NowList, TaskView};

/// Words that carry no meaning in a recall question ("what did we say about…").
const STOP_WORDS: &[&str] = &[
    "a", "about", "an", "and", "any", "are", "as", "at", "be", "can", "cover", "covered", "did",
    "do", "does", "for", "from", "how", "i", "in", "is", "it", "last", "me", "my", "of", "on",
    "or", "our", "say", "said", "show", "that", "the", "this", "time", "to", "us", "was", "we",
    "what", "when", "where", "which", "who", "why", "with", "you",
];

/// One recall hit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RecallHit {
    pub kind: String,
    pub reference: String,
    pub path: String,
    pub title: String,
    pub snippet: String,
    pub session_date: Option<String>,
}

/// The FTS5 expression for a question: its meaningful words, each quoted (so no word can be
/// read as an operator), any of them matching. BM25 ranks documents with more and rarer words
/// first. `None` when no word is left.
pub(crate) fn match_expression(query: &str) -> Option<String> {
    let mut terms: Vec<String> = Vec::new();
    for word in query.split(|character: char| !character.is_alphanumeric()) {
        let word = word.to_lowercase();
        if word.chars().count() < 2 || STOP_WORDS.contains(&word.as_str()) || terms.contains(&word)
        {
            continue;
        }
        terms.push(word);
        if terms.len() == RECALL_QUERY_TERMS_MAX {
            break;
        }
    }
    (!terms.is_empty()).then(|| {
        terms
            .iter()
            .map(|term| format!("\"{term}\""))
            .collect::<Vec<_>>()
            .join(" OR ")
    })
}

/// The best hits for a question, and how many more matched.
pub(crate) fn recall(
    connection: &Connection,
    expression: &str,
) -> Result<(Vec<RecallHit>, u64), StoreError> {
    let total: i64 = connection.query_row(
        "SELECT count(*) FROM search WHERE search MATCH ?1",
        [expression],
        |row| row.get(0),
    )?;
    // The snippet is 16 tokens around the match in the body; a match in a title ranks four
    // times higher than one in a body.
    let mut statement = connection.prepare(
        "SELECT search.kind, search.ref, search.path, search.title,
                snippet(search, 1, '«', '»', '…', 16), sessions.started
         FROM search
         LEFT JOIN sessions ON sessions.id = search.ref
              AND search.kind IN ('digest', 'transcript')
         WHERE search MATCH ?1
         ORDER BY bm25(search, 4.0, 1.0)
         LIMIT ?2",
    )?;
    let rows = statement.query_map(
        params![
            expression,
            i64::try_from(RECALL_HITS_MAX).unwrap_or(i64::MAX)
        ],
        |row| {
            Ok(RecallHit {
                kind: row.get(0)?,
                reference: row.get(1)?,
                path: row.get(2)?,
                title: row.get(3)?,
                snippet: row.get(4)?,
                session_date: row
                    .get::<_, Option<String>>(5)?
                    .map(|started| date_of(&started))
                    .filter(|date| !date.is_empty()),
            })
        },
    )?;
    let hits = rows.collect::<Result<Vec<_>, _>>()?;
    let more = u64::try_from(total)
        .unwrap_or(0)
        .saturating_sub(hits.len() as u64);
    Ok((hits, more))
}

/// Renders recall hits for the talker, within `RECALL_TEXT_MAX_BYTES`.
pub(crate) fn recall_text(hits: &[RecallHit], more: u64) -> String {
    if hits.is_empty() {
        return "Nothing in your notes, digests, memories, tasks or past sessions matches."
            .to_owned();
    }
    let mut out = String::from("Found in your vault:\n");
    let mut left_out = more;
    for (number, hit) in hits.iter().enumerate() {
        let source = match hit.kind.as_str() {
            "digest" | "transcript" => format!(
                "session {}{}",
                hit.reference,
                hit.session_date
                    .as_deref()
                    .map(|date| format!(", {date}"))
                    .unwrap_or_default()
            ),
            _ => hit.path.clone(),
        };
        let line = format!(
            "{}. [{}] {} ({source}): {}\n",
            number + 1,
            hit.kind,
            hit.title,
            hit.snippet.replace('\n', " ")
        );
        if out.len() + line.len() > RECALL_TEXT_MAX_BYTES {
            left_out += 1;
            continue;
        }
        out.push_str(&line);
    }
    if left_out > 0 {
        out.push_str(&format!("({left_out} more not shown)\n"));
    }
    out
}

/// Sessions, newest first.
pub(crate) fn library(connection: &Connection) -> Result<Library, StoreError> {
    let total: i64 = connection.query_row("SELECT count(*) FROM sessions", [], |row| row.get(0))?;
    let mut statement = connection.prepare(
        "SELECT id, folder, title, started, duration_ms, has_board FROM sessions
         ORDER BY started_ms DESC, id DESC LIMIT ?1",
    )?;
    let rows = statement.query_map(
        [i64::try_from(LIBRARY_ENTRIES_MAX).unwrap_or(i64::MAX)],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, bool>(5)?,
            ))
        },
    )?;
    let mut sessions = Vec::new();
    for row in rows {
        let (id, folder, title, started, duration_ms, has_board) = row?;
        let Some(id) = SessionId::parse(&id) else {
            continue;
        };
        let cover = has_board
            .then(|| VaultPath::session_file(&folder, BOARD_FILE))
            .flatten()
            .map(|path| path.to_string());
        sessions.push(LibraryEntry {
            id,
            title,
            date: date_of(&started),
            started,
            duration_ms: u64::try_from(duration_ms).unwrap_or(0),
            cover,
            shapes: Vec::new(),
        });
    }
    let more_sessions = u64::try_from(total)
        .unwrap_or(0)
        .saturating_sub(sessions.len() as u64);
    Ok(Library {
        sessions,
        more_sessions,
    })
}

/// Open tasks, by project then file order. A project matches by name (any case) or by its
/// file name, so "harness" finds `projects/harness.md` titled "Harness project".
pub(crate) fn now_list(
    connection: &Connection,
    project: Option<&str>,
) -> Result<NowList, StoreError> {
    let project_path = project
        .and_then(slugify)
        .and_then(|slug| VaultPath::project(&slug))
        .map(|path| path.to_string());
    let filter = "tasks.done = 0 AND (?1 IS NULL OR tasks.project = ?1 COLLATE NOCASE \
                  OR tasks.path = ?2)";
    let total: i64 = connection.query_row(
        &format!("SELECT count(*) FROM tasks WHERE {filter}"),
        params![project, project_path],
        |row| row.get(0),
    )?;
    let mut statement = connection.prepare(&format!(
        "SELECT tasks.id, tasks.title, tasks.project, tasks.done, tasks.links, tasks.session,
                sessions.title, sessions.started
         FROM tasks LEFT JOIN sessions ON sessions.id = tasks.session
         WHERE {filter}
         ORDER BY tasks.project COLLATE NOCASE, tasks.path, tasks.position LIMIT ?3"
    ))?;
    let rows = statement.query_map(
        params![
            project,
            project_path,
            i64::try_from(NOW_LIST_MAX).unwrap_or(i64::MAX)
        ],
        |row| {
            Ok(TaskView {
                id: row.get(0)?,
                title: row.get(1)?,
                project: row.get(2)?,
                done: row.get(3)?,
                links: serde_json::from_str(&row.get::<_, String>(4)?).unwrap_or_default(),
                session: row.get(5)?,
                session_title: row.get(6)?,
                session_date: row
                    .get::<_, Option<String>>(7)?
                    .map(|started| date_of(&started)),
            })
        },
    )?;
    let tasks = rows.collect::<Result<Vec<_>, _>>()?;
    Ok(NowList {
        project: project.map(str::to_owned),
        more_tasks: u64::try_from(total)
            .unwrap_or(0)
            .saturating_sub(tasks.len() as u64),
        tasks,
    })
}

/// A session as the index knows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionRecord {
    pub id: SessionId,
    pub folder: String,
    pub title: String,
    pub started: String,
    pub duration_ms: u64,
    pub summary: String,
}

/// Finds a session by id, or else by the words of `query` in session titles and digests.
pub(crate) fn find_session(
    connection: &Connection,
    query: &str,
) -> Result<Option<SessionRecord>, StoreError> {
    let by_id = |id: &str| -> Result<Option<SessionRecord>, StoreError> {
        Ok(connection
            .query_row(
                "SELECT id, folder, title, started, duration_ms, summary FROM sessions
                 WHERE id = ?1",
                [id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .optional()?
            .and_then(|(id, folder, title, started, duration_ms, summary)| {
                Some(SessionRecord {
                    id: SessionId::parse(&id)?,
                    folder,
                    title,
                    started,
                    duration_ms: u64::try_from(duration_ms).unwrap_or(0),
                    summary,
                })
            }))
    };
    if let Some(found) = by_id(query.trim())? {
        return Ok(Some(found));
    }
    let Some(expression) = match_expression(query) else {
        return Ok(None);
    };
    let best: Option<String> = connection
        .query_row(
            "SELECT ref FROM search WHERE search MATCH ?1 AND kind IN ('digest', 'transcript')
             ORDER BY bm25(search, 4.0, 1.0) LIMIT 1",
            [expression],
            |row| row.get(0),
        )
        .optional()?;
    match best {
        Some(id) => by_id(&id),
        None => Ok(None),
    }
}

/// "Where we left off": the latest digest's title, first two sentences and first open
/// question, within `RECAP_MAX_CHARS`.
pub(crate) fn recap(connection: &Connection) -> Result<Option<String>, StoreError> {
    let latest = connection
        .query_row(
            "SELECT title, started, summary, open_questions FROM sessions WHERE has_digest = 1
             ORDER BY started_ms DESC, id DESC LIMIT 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()?;
    Ok(latest.map(|(title, started, summary, questions)| {
        let when = WallTime::parse_rfc3339(&started)
            .map(|time| format!(" ({})", time.date()))
            .unwrap_or_default();
        let mut recap = format!("Last time{when}, \"{title}\": {}", two_sentences(&summary));
        if let Some(question) = questions.lines().find(|line| !line.trim().is_empty()) {
            recap.push_str(&format!(" Still open: {}", question.trim()));
        }
        cut_chars(&recap, RECAP_MAX_CHARS)
    }))
}

/// The first two sentences of a summary.
pub(crate) fn two_sentences(text: &str) -> String {
    let mut ends = text
        .char_indices()
        .filter(|&(at, character)| {
            matches!(character, '.' | '!' | '?')
                && text[at + character.len_utf8()..]
                    .chars()
                    .next()
                    .is_none_or(char::is_whitespace)
        })
        .map(|(at, character)| at + character.len_utf8());
    let end = ends.nth(1).unwrap_or(text.len());
    text[..end].trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::{match_expression, two_sentences};

    #[test]
    fn questions_become_quoted_terms() {
        assert_eq!(
            match_expression("What did we cover on consensus?").as_deref(),
            Some("\"consensus\"")
        );
        assert_eq!(
            match_expression("Redis OR \"TTL\" NEAR(x)").as_deref(),
            Some("\"redis\" OR \"ttl\" OR \"near\"")
        );
        assert_eq!(match_expression("what did we say?"), None);
        assert_eq!(match_expression(""), None);
    }

    #[test]
    fn recaps_keep_two_sentences() {
        assert_eq!(two_sentences("One. Two! Three?"), "One. Two!");
        assert_eq!(
            two_sentences("v1.2 is out. Then x."),
            "v1.2 is out. Then x."
        );
        assert_eq!(two_sentences("No end"), "No end");
    }
}
