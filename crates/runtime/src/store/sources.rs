//! A project's sources in the vault (D-184, D-185): the store is their only writer (law 3).
//!
//! ```text
//! projects/<id>/sources.md                    what the project keeps, one line per source
//! projects/<id>/briefs/<source>--<topic>.md   a cached research brief, with its stamp
//! ```
//!
//! `sources.md` is yours to edit: the store appends a line or removes one and leaves every other
//! byte as written (D-042). A line is `- <kind>: <path>`, the path as written (`~/…` under home).
//! Briefs are the store's own files, written whole and redacted (I8). The vault is the source of
//! truth; nothing about sources is indexed in SQLite: the runtime reads them when a project
//! becomes current. The session's project goes into its digest's front matter, so the next
//! session starts with it.

use boardai_core::sources::{DocumentFormat, Project, ProjectId, SourceKind};
use tokio::sync::oneshot;

use super::actor::{Message, Reply};
use super::error::StoreError;
use super::index::views::latest_session_folder;
use super::read::Reader;
use super::redact::redact_text;
use super::state::{SessionSlot, StoreState};
use super::vault::edit::{Editable, Edited, read_editable};
use super::vault::markdown::{FrontValue, render_front_matter, split_front_matter};
use super::vault::{
    DIGEST_FILE, FileRead, PROJECTS, VaultPath, read_text, remove_file, write_atomic,
};
use super::{Store, slug::slugify};
use crate::limits::{
    PROJECT_BRIEF_FILES_SCANNED_MAX, PROJECT_FILE_READ_MAX_BYTES, SESSION_BRIEFS_CACHED_MAX,
};

const SOURCES_FILE: &str = "sources.md";
const BRIEFS_FOLDER: &str = "briefs";

/// A source a project keeps, as its line in `sources.md` says it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeptLine {
    pub kind: SourceKind,
    /// As written: `~/…` under the home folder, else absolute.
    pub path: String,
}

/// A cached brief in the project's folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredBrief {
    /// The source's path as written.
    pub source: String,
    /// The topic's cache key (`overview`, `component-arena`).
    pub topic: String,
    /// What the code looked like when it was written: the git HEAD and the newest file time.
    pub stamp: String,
    /// `YYYY-MM-DD`.
    pub written: String,
    pub markdown: String,
}

/// The requests about sources, through the actor's one inbox.
pub(crate) enum SourcesRequest {
    Kept {
        project: ProjectId,
        reply: Reply<Vec<KeptLine>>,
    },
    Keep {
        project: Project,
        line: KeptLine,
        reply: Reply<()>,
    },
    Unkeep {
        project: ProjectId,
        path: String,
        reply: Reply<()>,
    },
    SaveBrief {
        project: ProjectId,
        brief: StoredBrief,
        reply: Reply<()>,
    },
    Briefs {
        project: ProjectId,
        source: String,
        reply: Reply<Vec<StoredBrief>>,
    },
    DeleteBrief {
        project: ProjectId,
        source: String,
        topic: String,
        reply: Reply<()>,
    },
    SessionProject {
        project: Option<Project>,
        reply: Reply<()>,
    },
}

impl Store {
    /// What a project keeps, from its `sources.md`; none when it keeps nothing yet.
    pub async fn project_sources(&self, project: &ProjectId) -> Result<Vec<KeptLine>, StoreError> {
        let project = project.clone();
        self.request(|reply| Message::Sources(SourcesRequest::Kept { project, reply }))
            .await
    }

    /// Keeps a source in a project: a line appended to its `sources.md` (made when missing).
    /// Keeping one already listed changes nothing.
    pub async fn keep_source(&self, project: &Project, line: KeptLine) -> Result<(), StoreError> {
        let project = project.clone();
        self.request(|reply| {
            Message::Sources(SourcesRequest::Keep {
                project,
                line,
                reply,
            })
        })
        .await
    }

    /// Stops keeping the source at `path` (as written): its line is removed.
    pub async fn unkeep_source(&self, project: &ProjectId, path: &str) -> Result<(), StoreError> {
        let (project, path) = (project.clone(), path.to_owned());
        self.request(|reply| {
            Message::Sources(SourcesRequest::Unkeep {
                project,
                path,
                reply,
            })
        })
        .await
    }

    pub async fn save_brief(
        &self,
        project: &ProjectId,
        brief: StoredBrief,
    ) -> Result<(), StoreError> {
        let project = project.clone();
        self.request(|reply| {
            Message::Sources(SourcesRequest::SaveBrief {
                project,
                brief,
                reply,
            })
        })
        .await
    }

    /// The cached briefs of one kept source.
    pub async fn project_briefs(
        &self,
        project: &ProjectId,
        source: &str,
    ) -> Result<Vec<StoredBrief>, StoreError> {
        let (project, source) = (project.clone(), source.to_owned());
        self.request(|reply| {
            Message::Sources(SourcesRequest::Briefs {
                project,
                source,
                reply,
            })
        })
        .await
    }

    pub async fn delete_brief(
        &self,
        project: &ProjectId,
        source: &str,
        topic: &str,
    ) -> Result<(), StoreError> {
        let (project, source, topic) = (project.clone(), source.to_owned(), topic.to_owned());
        self.request(|reply| {
            Message::Sources(SourcesRequest::DeleteBrief {
                project,
                source,
                topic,
                reply,
            })
        })
        .await
    }

    /// The project this session works on, recorded in its digest for the next session.
    pub async fn set_session_project(&self, project: Option<Project>) -> Result<(), StoreError> {
        self.request(|reply| Message::Sources(SourcesRequest::SessionProject { project, reply }))
            .await
    }

    /// The project the latest session with a digest worked on.
    pub async fn last_project(&self) -> Result<Option<Project>, StoreError> {
        let (reply, response) = oneshot::channel();
        self.inbox
            .send(Message::Read(super::actor::ReadRequest::LastProject {
                reply,
            }))
            .await
            .map_err(|_| StoreError::Stopped)?;
        response.await.map_err(|_| StoreError::Stopped)?
    }
}

/// Serves one request on the state, on the actor's blocking thread.
pub(crate) fn handle(state: &mut StoreState, request: SourcesRequest) {
    match request {
        SourcesRequest::Kept { project, reply } => {
            let _ = reply.send(kept(state, &project));
        }
        SourcesRequest::Keep {
            project,
            line,
            reply,
        } => {
            let _ = reply.send(keep(state, &project, &line));
        }
        SourcesRequest::Unkeep {
            project,
            path,
            reply,
        } => {
            let _ = reply.send(unkeep(state, &project, &path));
        }
        SourcesRequest::SaveBrief {
            project,
            brief,
            reply,
        } => {
            let _ = reply.send(save_brief(state, &project, &brief));
        }
        SourcesRequest::Briefs {
            project,
            source,
            reply,
        } => {
            let _ = reply.send(briefs(state, &project, &source));
        }
        SourcesRequest::DeleteBrief {
            project,
            source,
            topic,
            reply,
        } => {
            let _ = reply.send(delete_brief(state, &project, &source, &topic));
        }
        SourcesRequest::SessionProject { project, reply } => {
            let _ = reply.send(session_project(state, project));
        }
    }
}

fn sources_file(project: &ProjectId) -> Result<VaultPath, StoreError> {
    VaultPath::join(&[PROJECTS, project.as_str(), SOURCES_FILE]).ok_or_else(|| {
        StoreError::NotFound {
            what: "a project folder".to_owned(),
        }
    })
}

/// `inf-alloc-3f9a01c2--overview.md`: the source's name and a hash of its path, so two folders
/// of one name never share a brief.
fn brief_file(project: &ProjectId, source: &str, topic: &str) -> Result<VaultPath, StoreError> {
    let name = source.rsplit('/').next().unwrap_or(source);
    let slug = slugify(name).unwrap_or_else(|| "source".to_owned());
    let topic = slugify(topic).unwrap_or_else(|| "topic".to_owned());
    let file = format!("{slug}-{:08x}--{topic}.md", path_hash(source));
    VaultPath::join(&[PROJECTS, project.as_str(), BRIEFS_FOLDER, &file]).ok_or_else(|| {
        StoreError::NotFound {
            what: "a brief's file name".to_owned(),
        }
    })
}

/// FNV-1a of the path, folded to 32 bits: stable across builds, unlike `DefaultHasher`.
fn path_hash(path: &str) -> u32 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0100_0000_01b3;
    let hash = path.bytes().fold(OFFSET, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(PRIME)
    });
    u32::try_from((hash >> 32) ^ (hash & 0xffff_ffff)).unwrap_or(u32::MAX)
}

fn kind_noun(kind: SourceKind) -> &'static str {
    kind.noun()
}

fn kind_of(noun: &str) -> Option<SourceKind> {
    [
        SourceKind::CodeFolder,
        SourceKind::CodeFile,
        SourceKind::Document {
            format: DocumentFormat::Markdown,
        },
        SourceKind::Document {
            format: DocumentFormat::Text,
        },
        SourceKind::Document {
            format: DocumentFormat::Pdf,
        },
    ]
    .into_iter()
    .find(|kind| kind.noun().eq_ignore_ascii_case(noun.trim()))
}

fn line_of(line: &KeptLine) -> String {
    format!("- {}: {}", kind_noun(line.kind), escaped(&line.path))
}

/// Text as one line of `sources.md`: a control character (a line break would start a line of
/// its own) is written as its escape (`\n`, `\u{7}`); everything else as it is. Paths are refused
/// with one where they enter (`sources::validate`); this holds a hand-edited path, or a
/// project's name, to its line.
fn escaped(text: &str) -> String {
    let mut line = String::with_capacity(text.len());
    for character in text.chars() {
        if character.is_control() {
            line.extend(character.escape_default());
        } else {
            line.push(character);
        }
    }
    line
}

/// The kept lines of a `sources.md`: `- <kind>: <path>`. Anything else is yours and ignored.
fn parse_kept(text: &str) -> Vec<KeptLine> {
    text.lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("- ")?;
            let (noun, path) = rest.split_once(": ")?;
            let path = path.trim();
            let kind = kind_of(noun)?;
            (!path.is_empty()).then(|| KeptLine {
                kind,
                path: path.to_owned(),
            })
        })
        .collect()
}

fn read_bounded(state: &StoreState, path: &VaultPath) -> Result<Option<String>, StoreError> {
    let bound = u64::try_from(PROJECT_FILE_READ_MAX_BYTES).unwrap_or(u64::MAX);
    match read_text(&state.vault.absolute(path), bound) {
        Ok(FileRead::Missing) => Ok(None),
        Ok(FileRead::Text { text, .. }) => Ok(Some(text)),
        Ok(FileRead::TooLarge { .. }) => Err(StoreError::Limit {
            what: "a project's sources file",
            max: PROJECT_FILE_READ_MAX_BYTES,
        }),
        Err(error) => Err(StoreError::io(path.as_str(), &error)),
    }
}

fn kept(state: &StoreState, project: &ProjectId) -> Result<Vec<KeptLine>, StoreError> {
    let path = sources_file(project)?;
    Ok(read_bounded(state, &path)?
        .map(|text| parse_kept(&text))
        .unwrap_or_default())
}

fn keep(state: &StoreState, project: &Project, line: &KeptLine) -> Result<(), StoreError> {
    let path = sources_file(&project.id)?;
    let absolute = state.vault.absolute(&path);
    let io = |error: std::io::Error| StoreError::io(path.as_str(), &error);
    let added = redact_text(&line_of(line));
    match read_editable(
        &absolute,
        u64::try_from(PROJECT_FILE_READ_MAX_BYTES).unwrap_or(u64::MAX),
    )
    .map_err(io)?
    {
        Editable::Missing => {
            let header = redact_text(&format!(
                "# {} · sources\n\nWhat BoardAI may read in every session of this project. \
                 Remove a line to stop sharing it.\n\n",
                escaped(&project.name)
            ));
            write_atomic(&absolute, &Edited::create_text(&header, &added)).map_err(io)
        }
        Editable::Text(file) => {
            let written = escaped(&line.path);
            if parse_kept(file.as_str())
                .iter()
                .any(|known| known.path == written)
            {
                return Ok(());
            }
            write_atomic(&absolute, &Edited::append_text(&file, &added)).map_err(io)
        }
        Editable::LeftAlone => Err(StoreError::Limit {
            what: "a project's sources file",
            max: PROJECT_FILE_READ_MAX_BYTES,
        }),
    }
}

fn unkeep(state: &StoreState, project: &ProjectId, path_written: &str) -> Result<(), StoreError> {
    let path = sources_file(project)?;
    let absolute = state.vault.absolute(&path);
    let io = |error: std::io::Error| StoreError::io(path.as_str(), &error);
    let bound = u64::try_from(PROJECT_FILE_READ_MAX_BYTES).unwrap_or(u64::MAX);
    let Editable::Text(file) = read_editable(&absolute, bound).map_err(io)? else {
        return Ok(());
    };
    let written = escaped(path_written);
    let edited = Edited::remove_lines(&file, |line| {
        parse_kept(line)
            .first()
            .is_some_and(|kept| kept.path == written)
    });
    match edited {
        Some(edited) => write_atomic(&absolute, &edited).map_err(io),
        None => Ok(()),
    }
}

fn save_brief(
    state: &StoreState,
    project: &ProjectId,
    brief: &StoredBrief,
) -> Result<(), StoreError> {
    let path = brief_file(project, &brief.source, &brief.topic)?;
    let text = format!(
        "{}{}",
        render_front_matter(&[
            ("source", FrontValue::Text(brief.source.clone())),
            ("topic", FrontValue::Text(brief.topic.clone())),
            ("stamp", FrontValue::Text(brief.stamp.clone())),
            ("written", FrontValue::Text(brief.written.clone())),
        ]),
        brief.markdown
    );
    write_atomic(&state.vault.absolute(&path), &redact_text(&text))
        .map_err(|error| StoreError::io(path.as_str(), &error))
}

fn briefs(
    state: &StoreState,
    project: &ProjectId,
    source: &str,
) -> Result<Vec<StoredBrief>, StoreError> {
    let Some(folder) = VaultPath::join(&[PROJECTS, project.as_str(), BRIEFS_FOLDER]) else {
        return Ok(Vec::new());
    };
    let prefix = brief_file(project, source, "x")?;
    let prefix = prefix
        .as_str()
        .rsplit('/')
        .next()
        .and_then(|name| name.split_once("--"))
        .map(|(head, _)| format!("{head}--"))
        .unwrap_or_default();
    let Ok(entries) = std::fs::read_dir(state.vault.absolute(&folder)) else {
        return Ok(Vec::new());
    };
    let mut found = Vec::new();
    // Bounded by the files looked at, then by the briefs kept: the source's own are filtered
    // first, so a project with many sources still finds every brief of this one.
    for entry in entries.flatten().take(PROJECT_BRIEF_FILES_SCANNED_MAX) {
        if found.len() >= SESSION_BRIEFS_CACHED_MAX {
            break;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(&prefix) || !name.ends_with(".md") {
            continue;
        }
        let Some(path) = VaultPath::join(&[PROJECTS, project.as_str(), BRIEFS_FOLDER, &name])
        else {
            continue;
        };
        let Ok(Some(text)) = read_bounded(state, &path) else {
            continue;
        };
        let (Some(front), body) = split_front_matter(&text) else {
            continue;
        };
        let field = |key: &str| front.text(key).unwrap_or_default().to_owned();
        if field("source") != source {
            continue;
        }
        found.push(StoredBrief {
            source: field("source"),
            topic: field("topic"),
            stamp: field("stamp"),
            written: field("written"),
            markdown: body.to_owned(),
        });
    }
    Ok(found)
}

fn delete_brief(
    state: &StoreState,
    project: &ProjectId,
    source: &str,
    topic: &str,
) -> Result<(), StoreError> {
    let path = brief_file(project, source, topic)?;
    remove_file(&state.vault.absolute(&path))
        .map(|_| ())
        .map_err(|error| StoreError::io(path.as_str(), &error))
}

fn session_project(state: &mut StoreState, project: Option<Project>) -> Result<(), StoreError> {
    match &mut state.slot {
        SessionSlot::Recording(session) | SessionSlot::Ended(session) => {
            session.project = project;
            Ok(())
        }
        SessionSlot::Idle | SessionSlot::Forgotten => Ok(()),
    }
}

/// The project of the latest session with a digest, from its front matter.
pub(crate) fn last_project(reader: &Reader) -> Result<Option<Project>, StoreError> {
    let Some(folder) = reader.query(latest_session_folder)? else {
        return Ok(None);
    };
    let Some(path) = VaultPath::session_file(&folder, DIGEST_FILE) else {
        return Ok(None);
    };
    let bound = u64::try_from(PROJECT_FILE_READ_MAX_BYTES).unwrap_or(u64::MAX);
    let Ok(FileRead::Text { text, .. }) = read_text(&reader.vault().absolute(&path), bound) else {
        return Ok(None);
    };
    let (Some(front), _) = split_front_matter(&text) else {
        return Ok(None);
    };
    let (Some(id), Some(name)) = (front.text("project"), front.text("project_name")) else {
        return Ok(None);
    };
    Ok(ProjectId::parse(id).map(|id| Project {
        id,
        name: name.to_owned(),
    }))
}

#[cfg(test)]
mod tests {
    use super::{KeptLine, brief_file, line_of, parse_kept};
    use boardai_core::sources::{DocumentFormat, ProjectId, SourceKind};

    /// Finding 21: a path with a line break writes one line, which reads back as one source;
    /// it never adds a line of its own. Other characters are kept as written.
    #[test]
    fn a_kept_line_is_one_line_whatever_its_path_holds() {
        let line = KeptLine {
            kind: SourceKind::CodeFolder,
            path: "~/notes\n- code folder: ~/.ssh\r\u{7}".into(),
        };
        let written = line_of(&line);
        assert_eq!(written.lines().count(), 1, "{written}");
        assert_eq!(
            written,
            "- code folder: ~/notes\\n- code folder: ~/.ssh\\r\\u{7}"
        );
        assert_eq!(parse_kept(&written).len(), 1);
        let accented = KeptLine {
            kind: SourceKind::CodeFolder,
            path: "~/Café/naïve \\ tool".into(),
        };
        assert_eq!(line_of(&accented), "- code folder: ~/Café/naïve \\ tool");
    }

    #[test]
    fn kept_lines_are_read_back_and_your_own_lines_are_ignored() {
        let text = "# infinitydb · sources\n\nMy notes about these.\n\n- code folder: ~/code/infinitydb\n\
                    - PDF: ~/Papers/hekaton.pdf\n- something else\n- telescope: ~/x\n";
        assert_eq!(
            parse_kept(text),
            vec![
                KeptLine {
                    kind: SourceKind::CodeFolder,
                    path: "~/code/infinitydb".into()
                },
                KeptLine {
                    kind: SourceKind::Document {
                        format: DocumentFormat::Pdf
                    },
                    path: "~/Papers/hekaton.pdf".into()
                },
            ]
        );
    }

    #[test]
    fn two_sources_of_one_name_never_share_a_brief() {
        let Some(project) = ProjectId::parse("infinitydb") else {
            panic!("a project id");
        };
        let first = brief_file(&project, "~/a/inf-alloc", "overview").map(|path| path.to_string());
        let second = brief_file(&project, "~/b/inf-alloc", "overview").map(|path| path.to_string());
        assert_ne!(first, second);
        let first = first.unwrap_or_default();
        assert!(
            first.starts_with("projects/infinitydb/briefs/inf-alloc-"),
            "{first}"
        );
        assert!(first.ends_with("--overview.md"), "{first}");
    }
}
