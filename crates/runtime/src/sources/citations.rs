//! Every `path:line` a brief cites is read natively before its chip may show (law 6, D-123,
//! D-185): with the repo tools' own reader, confined to the source (credential paths refused,
//! symlinks resolved), and bounded per citation. A citation that names nothing readable, or
//! lines past the file's end, is returned unverified and the core drops it.
//!
//! A citation outside the source but inside its reach (`../session/mod.rs:12`, D-226) is read
//! the same way, confined to the reach, only when the reach is inside an allowed code folder
//! (`SourceDesk::cited_beyond`); otherwise it is unverified. It keeps its path as cited.

use std::path::{Component, Path, PathBuf};

use boardai_core::sources::{Citation, CitedLines};
use boardai_core::tools::{ToolCall, ToolOutput};

use crate::tools::{RepoRoots, run_blocking};

/// What reading the citations found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Checked {
    pub read: Vec<CitedLines>,
    pub unverified: Vec<Citation>,
}

/// Reads each citation inside `folder` (the source's folder, or the folder of a single file).
/// With `only_file`, a citation of any other file is beyond the source: that source is one file.
/// A citation beyond the source is read inside `beyond` (the reach, canonical, like `folder`)
/// when given, else unverified. Blocking: call it on the blocking pool.
#[must_use]
pub fn check_citations(
    folder: &Path,
    only_file: Option<&str>,
    beyond: Option<&Path>,
    citations: &[Citation],
) -> Checked {
    let mut checked = Checked::default();
    let Ok(roots) = RepoRoots::open(&[folder.to_path_buf()]) else {
        checked.unverified = citations.to_vec();
        return checked;
    };
    let reach =
        beyond.and_then(|reach| Some((reach, RepoRoots::open(&[reach.to_path_buf()]).ok()?)));
    for citation in citations {
        let in_source = only_file.is_none_or(|file| citation.path == file);
        let read = in_source
            .then(|| read_cited(&roots, &citation.path, citation))
            .flatten()
            .or_else(|| {
                let (reach, reach_roots) = reach.as_ref()?;
                let inside = lexical(&folder.join(&citation.path));
                let path = inside
                    .strip_prefix(reach)
                    .ok()?
                    .to_string_lossy()
                    .into_owned();
                read_cited(reach_roots, &path, citation)
            });
        match read {
            Some(lines) => checked.read.push(lines),
            None => checked.unverified.push(citation.clone()),
        }
    }
    checked
}

/// `path` with `.` and `..` folded away, without asking the file system; where it leads is
/// checked by the reader (symlinks included).
fn lexical(path: &Path) -> PathBuf {
    let mut normal = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                normal.pop();
            }
            Component::CurDir => {}
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => normal.push(part),
        }
    }
    normal
}

/// Reads `path` (inside `roots`) for `citation`, which keeps its own spelling.
fn read_cited(roots: &RepoRoots, path: &str, citation: &Citation) -> Option<CitedLines> {
    let call = ToolCall::ReadFile {
        path: path.to_owned(),
        start_line: Some(citation.first_line),
        end_line: Some(citation.last_line),
    };
    let ToolOutput::File {
        first_line, lines, ..
    } = run_blocking(roots, &call).ok()?
    else {
        return None;
    };
    let count = u32::try_from(lines.len()).ok()?;
    let covers =
        first_line == citation.first_line && first_line.checked_add(count)? > citation.last_line;
    covers.then(|| CitedLines {
        path: citation.path.clone(),
        first_line,
        lines,
    })
}

#[cfg(test)]
mod tests {
    use boardai_core::sources::Citation;

    use super::check_citations;

    fn cite(text: &str) -> Citation {
        Citation::parse(text).expect("a citation")
    }

    #[test]
    fn only_lines_that_are_there_are_verified() {
        let root = std::env::temp_dir().join(format!("boardai-cite-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).expect("a folder");
        std::fs::create_dir_all(root.join(".ssh")).expect("a folder");
        std::fs::write(root.join("src/arena.rs"), "a\nb\nc\nd\n").expect("a file");
        std::fs::write(root.join(".ssh/id_ed25519"), "secret\n").expect("a file");
        let citations = [
            cite("src/arena.rs:2-3"),
            cite("src/arena.rs:4-9"),
            cite("src/gone.rs:1"),
            cite(".ssh/id_ed25519:1"),
        ];
        let checked = check_citations(&root, None, None, &citations);
        assert_eq!(checked.read.len(), 1);
        assert_eq!(checked.read[0].lines, vec!["b".to_owned(), "c".to_owned()]);
        assert_eq!(checked.unverified.len(), 3);
        let single = check_citations(&root, Some("src/other.rs"), None, &citations[..1]);
        assert!(
            single.read.is_empty(),
            "a one-file source verifies only its own file"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// D-226: a citation beyond the source but inside its repository is read when the caller
    /// gives the reach (inside an allowed code folder), never past the reach, and dropped
    /// without it.
    #[test]
    fn citations_beyond_the_source_are_read_only_inside_the_reach() {
        let root = std::env::temp_dir().join(format!("boardai-cite-reach-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("repo");
        let source = repo.join("crates/alloc");
        std::fs::create_dir_all(source.join("src")).expect("a folder");
        std::fs::create_dir_all(repo.join("crates/store/src")).expect("a folder");
        std::fs::write(source.join("src/arena.rs"), "a\nb\n").expect("a file");
        std::fs::write(repo.join("crates/store/src/log.rs"), "x\ny\nz\n").expect("a file");
        std::fs::write(root.join("outside.rs"), "secret\n").expect("a file");
        let repo = std::fs::canonicalize(&repo).expect("canonical");
        let source = std::fs::canonicalize(&source).expect("canonical");
        let citations = [
            cite("src/arena.rs:1"),
            cite("../store/src/log.rs:2-3"),
            cite("../../../outside.rs:1"),
        ];
        let checked = check_citations(&source, None, Some(&repo), &citations);
        let read: Vec<&str> = checked
            .read
            .iter()
            .map(|lines| lines.path.as_str())
            .collect();
        assert_eq!(
            read,
            ["src/arena.rs", "../store/src/log.rs"],
            "kept as cited"
        );
        assert_eq!(checked.read[1].lines, ["y".to_owned(), "z".to_owned()]);
        assert_eq!(checked.unverified, [cite("../../../outside.rs:1")]);
        let without = check_citations(&source, None, None, &citations);
        assert_eq!(without.read.len(), 1, "no reach given: only the source");
        let _ = std::fs::remove_dir_all(&root);
    }
}
