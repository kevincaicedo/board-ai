//! The coding harnesses on the settings page (LLD §3.5): whether Claude Code and pi are
//! installed, the version each printed when asked, and which one a task runs on.
//!
//! A version costs a process (`claude --version` starts Node), so the status only reports
//! versions already known; the settings page asks for them once (`setup_harness_versions`), and
//! the answers are kept for the run, by program path. The first-run screen and the app's launch
//! never wait on one.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use boardai_core::setup::{HarnessFound, HarnessName, HarnessesFound};
use boardai_runtime::harness::HarnessDiscovery;
use boardai_runtime::settings::HarnessChoice;

use crate::helper::run_bounded;

/// How long a harness's `--version` may run: a Node CLI can take a second to start. Past it the
/// harness shows as found, without a version.
pub const HARNESS_VERSION_TIMEOUT_MS: u64 = 3_000;

/// Programs whose version is kept: two harnesses, and a path that changed (a reinstall) once
/// each. Past it the oldest answer is forgotten and asked again when needed.
const HARNESS_VERSIONS_KEPT_MAX: usize = 4;

/// The longest version kept (`2.1.301`, `0.18.2-beta.1`); a longer word is not a version.
const HARNESS_VERSION_MAX_BYTES: usize = 32;

/// What each program's `--version` answered this run (`None`: it didn't, in time).
#[derive(Debug, Default)]
pub struct HarnessVersions {
    // Held only to read or replace an entry, never while a process runs.
    known: Mutex<Vec<(PathBuf, Option<String>)>>,
}

impl HarnessVersions {
    /// The answer kept for `program`; `None` outside when it was never asked.
    pub fn known(&self, program: &Path) -> Option<Option<String>> {
        let known = self
            .known
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        known
            .iter()
            .find(|(path, _)| path == program)
            .map(|(_, version)| version.clone())
    }

    pub fn keep(&self, program: PathBuf, version: Option<String>) {
        let mut known = self
            .known
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        known.retain(|(path, _)| *path != program);
        if known.len() >= HARNESS_VERSIONS_KEPT_MAX {
            known.remove(0);
        }
        known.push((program, version));
    }
}

/// Installed or not, with the version this run knows. Never runs a process.
pub fn installed(discovery: &HarnessDiscovery, versions: &HarnessVersions) -> Installed {
    let found = |program: Option<&PathBuf>| match program {
        None => HarnessFound::Missing,
        Some(path) => HarnessFound::Found {
            version: versions.known(path).flatten(),
        },
    };
    Installed {
        claude_code: found(discovery.claude_code.as_ref()),
        pi: found(discovery.pi.as_ref()),
    }
}

/// Asks each installed harness not asked yet this run for its version, both at once, and keeps
/// the answers. Blocks for at most [`HARNESS_VERSION_TIMEOUT_MS`].
pub fn ask_versions(discovery: &HarnessDiscovery, versions: &HarnessVersions) {
    let programs: Vec<&PathBuf> = [discovery.claude_code.as_ref(), discovery.pi.as_ref()]
        .into_iter()
        .flatten()
        .filter(|program| versions.known(program).is_none())
        .collect();
    std::thread::scope(|scope| {
        for program in programs {
            scope.spawn(move || versions.keep(program.clone(), ask_version(program)));
        }
    });
}

/// The harnesses on this Mac before settings pick the default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub claude_code: HarnessFound,
    pub pi: HarnessFound,
}

/// The harnesses as the screen shows them: `[harness] kind` picks the default among the
/// installed ones (`auto` is Claude Code first, as the session does).
pub fn found(installed: Installed, choice: HarnessChoice) -> HarnessesFound {
    let has = |found: &HarnessFound| match found {
        HarnessFound::Found { .. } => true,
        HarnessFound::Missing => false,
    };
    let (claude_code, pi) = (has(&installed.claude_code), has(&installed.pi));
    let default = match choice {
        HarnessChoice::Auto if claude_code => Some(HarnessName::ClaudeCode),
        HarnessChoice::Auto if pi => Some(HarnessName::Pi),
        HarnessChoice::ClaudeCode if claude_code => Some(HarnessName::ClaudeCode),
        HarnessChoice::Pi if pi => Some(HarnessName::Pi),
        HarnessChoice::Auto | HarnessChoice::ClaudeCode | HarnessChoice::Pi => None,
    };
    HarnessesFound {
        claude_code: installed.claude_code,
        pi: installed.pi,
        default,
    }
}

fn ask_version(program: &Path) -> Option<String> {
    let mut command = Command::new(program);
    command.arg("--version");
    let finished = run_bounded(command, Duration::from_millis(HARNESS_VERSION_TIMEOUT_MS))?;
    if !finished.success {
        tracing::info!(program = %program.display(), "a harness's --version failed");
        return None;
    }
    parse_version(&finished.stdout)
}

/// The first word of `--version`'s first line that reads as a version: `2.1.301 (Claude Code)`
/// → `2.1.301`, `pi v0.18.2` → `0.18.2`. Anything else is no version, never a guess.
pub fn parse_version(output: &str) -> Option<String> {
    let line = output.lines().find(|line| !line.trim().is_empty())?;
    line.split_whitespace().find_map(|word| {
        let word = word.strip_prefix('v').unwrap_or(word);
        let word = word.trim_end_matches([',', ';', ')']);
        let starts_with_digit = word
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_digit());
        let version_bytes = word
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'));
        let valid = starts_with_digit
            && word.contains('.')
            && version_bytes
            && word.len() <= HARNESS_VERSION_MAX_BYTES;
        valid.then(|| word.to_owned())
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use boardai_core::setup::{HarnessFound, HarnessName};
    use boardai_runtime::harness::HarnessDiscovery;
    use boardai_runtime::settings::HarnessChoice;

    use super::{HarnessVersions, Installed, ask_versions, found, installed, parse_version};

    #[test]
    fn versions_are_read_from_what_the_harness_printed() {
        assert_eq!(
            parse_version("2.1.301 (Claude Code)\n").as_deref(),
            Some("2.1.301")
        );
        assert_eq!(parse_version("pi v0.18.2\n").as_deref(), Some("0.18.2"));
        assert_eq!(
            parse_version("\nversion 1.0.0-beta.2, built today").as_deref(),
            Some("1.0.0-beta.2")
        );
        for none in ["", "Usage: pi [options]", "42", "v.", "1.2.3;rm -rf"] {
            assert_eq!(parse_version(none), None, "{none:?}");
        }
        let long = format!("1.{}", "0".repeat(40));
        assert_eq!(parse_version(&long), None);
    }

    #[test]
    fn a_missing_harness_is_never_the_default() {
        let found_one = || HarnessFound::Found { version: None };
        let only_pi = Installed {
            claude_code: HarnessFound::Missing,
            pi: found_one(),
        };
        assert_eq!(
            found(only_pi.clone(), HarnessChoice::Auto).default,
            Some(HarnessName::Pi)
        );
        assert_eq!(
            found(only_pi.clone(), HarnessChoice::ClaudeCode).default,
            None
        );
        assert_eq!(
            found(only_pi, HarnessChoice::Pi).default,
            Some(HarnessName::Pi)
        );
        let both = Installed {
            claude_code: found_one(),
            pi: found_one(),
        };
        assert_eq!(
            found(both, HarnessChoice::Auto).default,
            Some(HarnessName::ClaudeCode)
        );
        let none = Installed {
            claude_code: HarnessFound::Missing,
            pi: HarnessFound::Missing,
        };
        assert_eq!(found(none, HarnessChoice::Auto).default, None);
    }

    #[test]
    fn versions_are_asked_once_bounded_and_kept_per_program() {
        let versions = HarnessVersions::default();
        let discovery = HarnessDiscovery {
            claude_code: Some(PathBuf::from("/bin/echo")),
            pi: Some(PathBuf::from("/nonexistent/pi")),
        };
        let before = installed(&discovery, &versions);
        assert_eq!(before.claude_code, HarnessFound::Found { version: None });
        ask_versions(&discovery, &versions);
        // `echo --version` prints "--version": no version, and nothing invented.
        assert_eq!(versions.known(&PathBuf::from("/bin/echo")), Some(None));
        assert_eq!(
            versions.known(&PathBuf::from("/nonexistent/pi")),
            Some(None)
        );
        versions.keep(PathBuf::from("/bin/echo"), Some("2.1.301".to_owned()));
        // An answer kept is not asked for again: `echo` would answer no version.
        ask_versions(&discovery, &versions);
        let after = installed(&discovery, &versions);
        assert_eq!(
            after.claude_code,
            HarnessFound::Found {
                version: Some("2.1.301".to_owned())
            }
        );
        for index in 0..10 {
            versions.keep(PathBuf::from(format!("/bin/h{index}")), None);
        }
        assert_eq!(
            versions.known(&PathBuf::from("/bin/echo")),
            None,
            "the oldest answer is forgotten past the bound"
        );
    }
}
