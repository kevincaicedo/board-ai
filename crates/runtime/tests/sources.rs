//! Sources in the runtime (D-184, D-185): what a dropped or chosen path must pass, and a
//! project's sources and briefs in the vault, with the project the next session starts with.
#![allow(clippy::expect_used, reason = "test setup fails loudly")]

mod store_support;
mod support;

use std::path::Path;

use boardai_core::sources::{DocumentFormat, Project, SourceKind};
use boardai_runtime::cancel::CancelToken;
use boardai_runtime::sources::{Refusal, validate};
use boardai_runtime::store::{KeptLine, SessionFinished, StoredBrief};
use store_support::{TestStore, at, caption, close};
use support::TempFolder;

fn refused(path: &Path, home: &Path) -> Option<Refusal> {
    validate(path, home).err()
}

/// A home folder with a code folder, documents, a photo, credentials, app data, a file too
/// large to be code, and a symlink into the credentials.
fn drop_tree() -> (TempFolder, std::path::PathBuf, std::path::PathBuf) {
    let folder = TempFolder::new("drop");
    let home = folder.path.join("home");
    let code = home.join("code/inf-alloc");
    std::fs::create_dir_all(code.join("src")).expect("a folder");
    std::fs::write(code.join("src/arena.rs"), "pub struct Arena;\n").expect("a file");
    std::fs::write(code.join("README.md"), "# inf-alloc\n").expect("a file");
    std::fs::write(home.join("paper.pdf"), "%PDF-1.7\n").expect("a file");
    std::fs::write(home.join("photo.png"), "png").expect("a file");
    std::fs::create_dir_all(home.join(".ssh")).expect("a folder");
    std::fs::write(home.join(".ssh/id_ed25519"), "secret\n").expect("a file");
    std::fs::create_dir_all(home.join("Library/Application Support/x")).expect("a folder");
    std::fs::write(home.join("Library/Application Support/x/notes.md"), "#\n").expect("a file");
    let big = std::fs::File::create(code.join("src/generated.rs")).expect("a file");
    big.set_len(5 * 1024 * 1024).expect("a large file");
    std::os::unix::fs::symlink(home.join(".ssh"), code.join("keys")).expect("a symlink");
    (folder, home, code)
}

/// The drop's checks (D-184): a folder is a code folder, a file's kind is its extension, and the
/// tray shows each under home as `~/…`.
#[test]
fn a_dropped_folder_or_file_gets_its_kind() {
    let (_folder, home, code) = drop_tree();
    let folder_source = validate(&code, &home).expect("a code folder");
    assert_eq!(folder_source.kind, SourceKind::CodeFolder);
    assert_eq!(folder_source.title, "inf-alloc");
    assert_eq!(folder_source.shown, "~/code/inf-alloc");
    assert!(folder_source.files_estimate.is_some_and(|count| count >= 2));
    let readme = validate(&code.join("README.md"), &home).expect("a document");
    assert_eq!(
        readme.kind,
        SourceKind::Document {
            format: DocumentFormat::Markdown
        }
    );
    assert_eq!(
        validate(&home.join("paper.pdf"), &home).map(|valid| valid.kind),
        Ok(SourceKind::Document {
            format: DocumentFormat::Pdf
        })
    );
    assert_eq!(
        validate(&code.join("src/arena.rs"), &home).map(|valid| valid.kind),
        Ok(SourceKind::CodeFile)
    );
}

/// What a drop must never share: a relative path, the disk, the home folder or above, `~/Library`,
/// credentials (by where a symlink leads too), a kind BoardAI doesn't read, a file past its size,
/// a path that is gone or too long.
#[test]
fn a_dropped_path_that_fails_a_check_is_refused_with_its_reason() {
    let (folder, home, code) = drop_tree();
    for (path, refusal) in [
        (
            std::path::PathBuf::from("relative/path"),
            Refusal::NotAbsolute,
        ),
        (std::path::PathBuf::from("/"), Refusal::HoldsEverything),
        (home.clone(), Refusal::HoldsEverything),
        (folder.path.clone(), Refusal::HoldsEverything),
        (home.join("Library"), Refusal::AppData),
        (
            home.join("Library/Application Support/x/notes.md"),
            Refusal::AppData,
        ),
        (home.join(".ssh"), Refusal::Credentials),
        (home.join(".ssh/id_ed25519"), Refusal::Credentials),
        (code.join("keys"), Refusal::Credentials),
        (home.join("photo.png"), Refusal::NotSupported),
        (code.join("src/generated.rs"), Refusal::TooLarge),
        (home.join("gone"), Refusal::Missing),
        (home.join("x".repeat(2_000)), Refusal::TooLong),
    ] {
        assert_eq!(refused(&path, &home), Some(refusal), "{}", path.display());
    }
}

fn kept(path: &str) -> KeptLine {
    KeptLine {
        kind: SourceKind::CodeFolder,
        path: path.to_owned(),
    }
}

/// A project's sources in the vault (D-184): `sources.md` gains and loses one line at a time,
/// with your own lines untouched.
#[tokio::test]
async fn a_project_keeps_its_sources_in_its_vault_folder() {
    let folder = TempFolder::new("project-sources");
    let test = TestStore::new(&folder.path);
    let (store, task, _) = test.open().await;
    let project = Project::named("InfinityDB").expect("a project");
    assert!(
        store
            .project_sources(&project.id)
            .await
            .expect("read")
            .is_empty()
    );
    store
        .keep_source(&project, kept("~/code/infinitydb"))
        .await
        .expect("kept");
    store
        .keep_source(&project, kept("~/code/infinitydb"))
        .await
        .expect("kept again");
    let file = "projects/infinitydb/sources.md";
    let written = test.read(file);
    assert!(written.starts_with("# InfinityDB · sources\n"), "{written}");
    assert_eq!(
        written.matches("- code folder: ~/code/infinitydb").count(),
        1
    );
    let yours = format!("{written}My own note about the project.\n");
    std::fs::write(test.vault().join(file), &yours).expect("edited by hand");
    store
        .keep_source(&project, kept("~/code/inf-alloc"))
        .await
        .expect("kept");
    store
        .unkeep_source(&project.id, "~/code/infinitydb")
        .await
        .expect("unkept");
    let after = test.read(file);
    assert!(
        after.contains("My own note about the project.\n"),
        "{after}"
    );
    assert!(!after.contains("~/code/infinitydb\n"), "{after}");
    let lines = store.project_sources(&project.id).await.expect("read");
    assert_eq!(lines, vec![kept("~/code/inf-alloc")]);
    close(store, task).await;
}

/// A kept source's briefs are files of their own in the project's folder, read back by their
/// source's path only, and deleted one at a time.
#[tokio::test]
async fn a_project_keeps_its_sources_briefs_beside_them() {
    let folder = TempFolder::new("project-briefs");
    let test = TestStore::new(&folder.path);
    let (store, task, _) = test.open().await;
    let project = Project::named("InfinityDB").expect("a project");
    let brief = StoredBrief {
        source: "~/code/inf-alloc".into(),
        topic: "overview".into(),
        stamp: "head=aaaa;newest=1;files=2".into(),
        written: "2026-10-15".into(),
        markdown: "# Brief: inf-alloc\n## Overview\nIt allocates.\n".into(),
    };
    store
        .save_brief(&project.id, brief.clone())
        .await
        .expect("saved");
    let other = StoredBrief {
        source: "~/elsewhere/inf-alloc".into(),
        ..brief.clone()
    };
    store.save_brief(&project.id, other).await.expect("saved");
    let briefs = store
        .project_briefs(&project.id, "~/code/inf-alloc")
        .await
        .expect("read");
    assert_eq!(
        briefs,
        vec![brief],
        "a source's briefs only, read back whole"
    );
    store
        .delete_brief(&project.id, "~/code/inf-alloc", "overview")
        .await
        .expect("deleted");
    assert!(
        store
            .project_briefs(&project.id, "~/code/inf-alloc")
            .await
            .expect("read")
            .is_empty()
    );
    close(store, task).await;
}

/// The project a session worked on goes into its digest, and the next session starts with it.
#[tokio::test]
async fn the_next_session_starts_with_the_last_sessions_project() {
    let folder = TempFolder::new("last-project");
    let test = TestStore::new(&folder.path);
    let (store, task, _) = test.open().await;
    assert_eq!(store.last_project().await.expect("read"), None);
    store.start_session(None).await.expect("starts");
    store.log_ui(
        at(1_000),
        vec![caption(boardai_core::protocol::Speaker::User, "Hi")],
    );
    let project = Project::named("infinitydb").expect("a project");
    store
        .set_session_project(Some(project.clone()))
        .await
        .expect("set");
    let finished = store
        .finish_session(at(5_000), None, None, CancelToken::new())
        .await
        .expect("finished");
    assert!(matches!(finished, SessionFinished::Written { .. }));
    assert_eq!(store.last_project().await.expect("read"), Some(project));
    close(store, task).await;
    // The vault is the source of truth: a rebuilt index finds the same project.
    let (store, task, _) = test.open().await;
    store.rebuild_index().await.expect("rebuilt");
    assert_eq!(
        store
            .last_project()
            .await
            .expect("read")
            .map(|project| project.name),
        Some("infinitydb".to_owned())
    );
    close(store, task).await;
}
