//! Repo tools against a temporary repo: confinement, `.gitignore`, binary files and the caps.
#![allow(clippy::expect_used, reason = "test setup fails loudly")]

mod support;

use std::os::unix::fs::symlink;
use std::path::PathBuf;

use boardai_core::RepoReads;
use boardai_core::tools::{DirEntry, ToolCall, ToolFailure, ToolOutput};
use boardai_runtime::limits::{SEARCH_HITS_MAX, TOOL_RESULT_MAX_BYTES, TOOL_RESULT_MAX_LINES};
use boardai_runtime::tools::{RepoRoots, RepoTools, ToolError, run_blocking};
use support::TempFolder;

struct Fixture {
    _dir: TempFolder,
    repo: PathBuf,
    outside: PathBuf,
    roots: RepoRoots,
}

fn fixture() -> Fixture {
    let folder = TempFolder::new("tools");
    let numbered: String = (1..=10).map(|n| format!("line {n} of lb\n")).collect();
    folder.write(
        "repo/src/lb.rs",
        format!("fn route() {{}}\n{numbered}").as_bytes(),
    );
    folder.write("repo/src/main.rs", b"fn main() {\n    route();\n}\n");
    let big: String = (1..=1_000).map(|n| format!("big line {n}\n")).collect();
    folder.write("repo/big.txt", big.as_bytes());
    let wide: String = (1..=500)
        .map(|_| format!("{}\n", "w".repeat(200)))
        .collect();
    folder.write("repo/wide.txt", wide.as_bytes());
    folder.write("repo/.gitignore", b"target/\n*.log\n");
    folder.write("repo/target/debug.rs", b"fn route() {}\n");
    folder.write("repo/app.log", b"fn route() {}\n");
    folder.write("repo/.git/config", b"fn route() {}\n");
    folder.write("repo/.github/ci.yml", b"run: fn route\n");
    folder.write("repo/blob.dat", b"fn route\0binary");
    folder.write("outside/secret.txt", b"fn route() { secret }\n");
    let repo = folder.path.join("repo");
    let outside = folder.path.join("outside");
    symlink(&outside, repo.join("link_out")).expect("symlink out");
    symlink(repo.join("src/lb.rs"), repo.join("link_in.rs")).expect("symlink in");
    let roots = RepoRoots::open(std::slice::from_ref(&repo)).expect("open repo");
    Fixture {
        _dir: folder,
        repo,
        outside,
        roots,
    }
}

fn read(
    roots: &RepoRoots,
    path: &str,
    start: Option<u32>,
    end: Option<u32>,
) -> Result<ToolOutput, ToolError> {
    run_blocking(
        roots,
        &ToolCall::ReadFile {
            path: path.into(),
            start_line: start,
            end_line: end,
        },
    )
}

fn failure(result: Result<ToolOutput, ToolError>) -> ToolFailure {
    match result {
        Err(error) => error.kind,
        Ok(output) => panic!("expected a failure, got {output:?}"),
    }
}

fn file(result: Result<ToolOutput, ToolError>) -> (String, u32, Vec<String>, u32) {
    match result {
        Ok(ToolOutput::File {
            path,
            first_line,
            lines,
            more_lines,
        }) => (path, first_line, lines, more_lines),
        other => panic!("expected a file, got {other:?}"),
    }
}

#[test]
fn reads_whole_files_and_line_ranges() {
    let fixture = fixture();
    let (path, first, lines, more) = file(read(&fixture.roots, "src/lb.rs", None, None));
    assert_eq!(
        (path.as_str(), first, lines.len(), more),
        ("src/lb.rs", 1, 11, 0)
    );
    let (_, first, lines, more) = file(read(&fixture.roots, "src/lb.rs", Some(3), Some(5)));
    assert_eq!(first, 3);
    assert_eq!(lines, ["line 2 of lb", "line 3 of lb", "line 4 of lb"]);
    assert_eq!(more, 0);
    let past_end = read(&fixture.roots, "src/lb.rs", Some(50), None);
    assert_eq!(failure(past_end), ToolFailure::InvalidInput);
}

#[test]
fn results_are_capped_by_lines_and_by_bytes() {
    let fixture = fixture();
    let (_, _, lines, more) = file(read(&fixture.roots, "big.txt", None, None));
    assert_eq!((lines.len(), more), (TOOL_RESULT_MAX_LINES, 600));
    assert_eq!(lines.last().map(String::as_str), Some("big line 400"));
    let (_, _, lines, more) = file(read(&fixture.roots, "big.txt", Some(901), Some(2_000)));
    assert_eq!((lines.len(), more), (100, 0));
    let (_, _, lines, more) = file(read(&fixture.roots, "wide.txt", None, None));
    let kept_bytes: usize = lines.iter().map(String::len).sum();
    assert!(kept_bytes <= TOOL_RESULT_MAX_BYTES);
    assert!(lines.len() < TOOL_RESULT_MAX_LINES);
    assert_eq!(lines.len() + more as usize, 500);
}

#[test]
fn paths_never_escape_the_open_folder() {
    let fixture = fixture();
    let secret = fixture.outside.join("secret.txt");
    let escapes = [
        "../outside/secret.txt".to_owned(),
        "src/../../outside/secret.txt".to_owned(),
        secret.to_string_lossy().into_owned(),
        "/etc/passwd".to_owned(),
        "link_out/secret.txt".to_owned(),
        "link_out".to_owned(),
    ];
    for escape in &escapes {
        assert_eq!(
            failure(read(&fixture.roots, escape, None, None)),
            ToolFailure::OutsideRepo,
            "{escape}"
        );
    }
    let listing = run_blocking(
        &fixture.roots,
        &ToolCall::ListDir {
            path: "link_out".into(),
        },
    );
    assert_eq!(failure(listing), ToolFailure::OutsideRepo);
    let (path, ..) = file(read(&fixture.roots, "link_in.rs", None, None));
    assert_eq!(path, "src/lb.rs");
    let absolute = fixture
        .repo
        .join("src/main.rs")
        .to_string_lossy()
        .into_owned();
    let (path, ..) = file(read(&fixture.roots, &absolute, None, None));
    assert_eq!(path, "src/main.rs");
}

#[test]
fn missing_binary_and_folder_paths_fail_with_their_kind() {
    let fixture = fixture();
    assert_eq!(
        failure(read(&fixture.roots, "nope.rs", None, None)),
        ToolFailure::NotFound
    );
    assert_eq!(
        failure(read(&fixture.roots, "blob.dat", None, None)),
        ToolFailure::InvalidInput
    );
    assert_eq!(
        failure(read(&fixture.roots, "src", None, None)),
        ToolFailure::InvalidInput
    );
    let none = RepoRoots::default();
    assert_eq!(
        failure(read(&none, "src/lb.rs", None, None)),
        ToolFailure::OutsideRepo
    );
}

fn search(roots: &RepoRoots, query: &str, glob: Option<&str>) -> Result<ToolOutput, ToolError> {
    run_blocking(
        roots,
        &ToolCall::SearchCode {
            query: query.into(),
            glob: glob.map(str::to_owned),
        },
    )
}

fn hit_paths(result: Result<ToolOutput, ToolError>) -> (Vec<String>, u32) {
    match result {
        Ok(ToolOutput::Matches { hits, more_hits }) => (
            hits.into_iter()
                .map(|hit| format!("{}:{}", hit.path, hit.line))
                .collect(),
            more_hits,
        ),
        other => panic!("expected matches, got {other:?}"),
    }
}

#[test]
fn search_respects_gitignore_and_skips_binaries_and_git() {
    let fixture = fixture();
    let (hits, more) = hit_paths(search(&fixture.roots, r"fn route", None));
    assert_eq!(hits, [".github/ci.yml:1", "src/lb.rs:1"]);
    assert_eq!(more, 0);
    let (hits, _) = hit_paths(search(&fixture.roots, r"route\(\)", Some("*.rs")));
    assert_eq!(hits, ["src/lb.rs:1", "src/main.rs:2"]);
    assert_eq!(
        failure(search(&fixture.roots, "(unclosed", None)),
        ToolFailure::InvalidInput
    );
}

#[test]
fn search_hits_are_capped_and_counted() {
    let fixture = fixture();
    let (hits, more) = hit_paths(search(&fixture.roots, r"^big line", None));
    assert_eq!(hits.len(), SEARCH_HITS_MAX);
    assert_eq!(more as usize, 1_000 - SEARCH_HITS_MAX);
}

#[test]
fn list_dir_shows_what_git_would_track() {
    let fixture = fixture();
    let listing = run_blocking(&fixture.roots, &ToolCall::ListDir { path: ".".into() });
    let Ok(ToolOutput::Listing {
        entries,
        more_entries,
    }) = listing
    else {
        panic!("expected a listing, got {listing:?}");
    };
    let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(
        names,
        [
            ".github",
            ".gitignore",
            "big.txt",
            "blob.dat",
            "link_in.rs",
            "link_out",
            "src",
            "wide.txt"
        ]
    );
    assert_eq!(more_entries, 0);
    assert!(entries.contains(&DirEntry {
        name: "src".into(),
        is_dir: true
    }));
}

#[tokio::test]
async fn tools_run_on_the_blocking_pool_and_refuse_other_tools() {
    let fixture = fixture();
    let tools = RepoTools::new(fixture.roots.clone());
    let output = tools.run(&ToolCall::ListDir { path: "src".into() }).await;
    assert!(matches!(output, ToolOutput::Listing { ref entries, .. } if entries.len() == 2));
    let output = tools.run(&ToolCall::Recall { query: "x".into() }).await;
    assert!(matches!(
        output,
        ToolOutput::Failed {
            kind: ToolFailure::InvalidInput,
            ..
        }
    ));
    let output = tools
        .run(&ToolCall::ReadFile {
            path: "../outside/secret.txt".into(),
            start_line: None,
            end_line: None,
        })
        .await;
    let ToolOutput::Failed { kind, message } = output else {
        panic!("expected a failure");
    };
    assert_eq!(kind, ToolFailure::OutsideRepo);
    assert!(!message.contains("secret }"));
}

/// A home folder opened by hand, with the usual credentials in it and a symlink into `.ssh`.
fn credential_home() -> (TempFolder, PathBuf, RepoRoots) {
    let folder = TempFolder::new("tools-credentials");
    let planted = b"fn route() { PLANTED-CREDENTIAL }\n";
    for credential in [
        "home/.ssh/id_ed25519",
        "home/.aws/credentials",
        "home/.gnupg/private-keys-v1.d/key",
        "home/.config/gh/hosts.yml",
        "home/.config/gcloud/credentials.db",
        "home/.azure/accessTokens.json",
        "home/.password-store/mail.gpg",
        "home/.kube/config",
        "home/.docker/config.json",
        "home/Library/Keychains/login.keychain-db",
        "home/.netrc",
        "home/.git-credentials",
    ] {
        folder.write(credential, planted);
    }
    folder.write("home/code/app.rs", b"fn route() {}\n");
    folder.write("home/.docker/compose.yml", b"fn route compose\n");
    folder.write("home/.config/nvim/init.lua", b"fn route nvim\n");
    let home = folder.path.join("home");
    symlink(home.join(".ssh"), home.join("code/keys")).expect("symlink to .ssh");
    let roots = RepoRoots::open(std::slice::from_ref(&home)).expect("open home");
    (folder, home, roots)
}

/// Review 2, finding 10: an opened folder can hold credentials (a home folder opened by hand
/// in `settings.toml`, a dotfiles repo). The repo tools never read, search or list them, and
/// a symlink into one is refused the same way.
#[test]
fn credential_folders_inside_an_opened_folder_are_never_read() {
    let (_folder, home, roots) = credential_home();
    for path in [
        ".ssh/id_ed25519",
        ".aws/credentials",
        ".config/gh/hosts.yml",
        ".kube/config",
        ".docker/config.json",
        "Library/Keychains/login.keychain-db",
        ".netrc",
        ".git-credentials",
        "code/keys/id_ed25519",
    ] {
        assert_eq!(
            failure(read(&roots, path, None, None)),
            ToolFailure::OutsideRepo,
            "{path} was read"
        );
    }
    let absolute = home.join(".ssh/id_ed25519");
    let absolute = absolute.to_string_lossy();
    assert_eq!(
        failure(read(&roots, &absolute, None, None)),
        ToolFailure::OutsideRepo
    );
    let (hits, _) = hit_paths(search(&roots, "fn route", None));
    assert_eq!(
        hits,
        [
            ".config/nvim/init.lua:1",
            ".docker/compose.yml:1",
            "code/app.rs:1"
        ],
        "a search never opens a credential file"
    );
    let listing = run_blocking(&roots, &ToolCall::ListDir { path: ".".into() });
    let Ok(ToolOutput::Listing { entries, .. }) = listing else {
        panic!("expected a listing, got {listing:?}");
    };
    let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(
        names,
        [".config", ".docker", ".kube", "Library", "code"],
        "a folder that only holds a credential file is listed; the file is not"
    );
    assert_eq!(
        failure(run_blocking(
            &roots,
            &ToolCall::ListDir {
                path: ".ssh".into()
            }
        )),
        ToolFailure::OutsideRepo
    );
}

/// The core asks for an announced read only when one could have been made (D-232): the tools
/// tell it so once a folder is open.
#[test]
fn the_repo_reads_are_offered_once_a_folder_is_open() {
    let fixture = fixture();
    assert_eq!(RepoTools::default().repo_reads(), RepoReads::NotOffered);
    assert_eq!(
        RepoTools::new(fixture.roots).repo_reads(),
        RepoReads::Offered
    );
}

/// D-291: BoardAI names a single shared file to the talker as plain text (square brackets made
/// round, fence markers broken up), so the file answers to that name as well as its own; a
/// file beside it still answers to nothing.
#[test]
fn a_single_shared_file_answers_to_the_name_the_talker_was_given() {
    let folder = TempFolder::new("plain-name");
    folder.write("docs/notes [draft].md", b"# Notes\nThe log first.\n");
    folder.write("docs/beside.md", b"# Beside\n");
    let shared = folder.path.join("docs/notes [draft].md");
    let roots =
        RepoRoots::open_with_files(&[], std::slice::from_ref(&shared)).expect("open the file");
    for name in ["notes [draft].md", "notes (draft).md"] {
        let (_, first, lines, _) = file(read(&roots, name, None, None));
        assert_eq!((first, lines.len()), (1, 2), "{name}");
    }
    assert_eq!(
        failure(read(&roots, "beside.md", None, None)),
        ToolFailure::NotFound
    );
}
