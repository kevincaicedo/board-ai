//! Microphone permission: the app's one entry point over the runtime's `AVAudioApplication`
//! calls (the FFI lives in `runtime::audio::coreaudio`, the only place `unsafe` is allowed).
//! The first-run screen asks through it, and so does audio startup when setup was skipped.
//!
//! macOS grants the microphone to the app responsible for a process, not to the process: run
//! from a terminal or an editor (`just dev` in Visual Studio Code's terminal), BoardAI's
//! microphone is that app's. [`attributed_to`] names it by walking the parent processes up to
//! the outermost one inside an `.app` bundle (D-149), so the screen can say which switch to
//! turn on, and [`open_microphone_settings`] opens that pane.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use boardai_core::setup::MicPermission;
use boardai_runtime::audio::{MicrophoneAccess, microphone_access, request_microphone_access};

/// How long a request waits for the user to answer macOS's prompt. Past it the request
/// returns whatever macOS reports then (still undetermined, most likely), so the first-run
/// card never waits forever on a prompt that didn't show.
pub const MICROPHONE_ANSWER_WAIT_MS: u64 = 120_000;

/// Parent processes walked to find the app macOS attributes the microphone to. A deeper chain
/// (never seen: `just dev` in an editor is about ten) names no app.
pub const ATTRIBUTION_ANCESTORS_MAX: usize = 32;

/// How long one `ps` or `open` may run before it is killed; the walk then names no app, and
/// opening the settings reports a failure.
pub const HELPER_PROCESS_TIMEOUT_MS: u64 = 1_000;

/// Output read from one helper process: `ps` prints one short line per process.
const HELPER_OUTPUT_MAX_BYTES: u64 = 4_096;

/// How often a running helper process is checked for its exit.
const HELPER_POLL_MS: u64 = 5;

/// The Privacy & Security › Microphone pane of System Settings.
const MICROPHONE_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone";

/// The current permission. Never prompts.
pub fn status() -> MicPermission {
    from_access(microphone_access())
}

/// Asks once (macOS shows its prompt only while undetermined) and returns the answer.
pub async fn request() -> MicPermission {
    let wait = Duration::from_millis(MICROPHONE_ANSWER_WAIT_MS);
    match tokio::time::timeout(wait, request_microphone_access()).await {
        Ok(answer) => from_access(answer),
        Err(_) => {
            tracing::warn!("no answer to the microphone prompt in time");
            status()
        }
    }
}

/// The permission, asking first while it is undetermined (audio startup after a dev script
/// skipped setup; after setup it is already answered).
pub async fn ask_once() -> MicPermission {
    match status() {
        MicPermission::Undetermined => request().await,
        known @ (MicPermission::Granted | MicPermission::Denied) => known,
    }
}

const fn from_access(access: MicrophoneAccess) -> MicPermission {
    match access {
        MicrophoneAccess::Granted => MicPermission::Granted,
        MicrophoneAccess::Denied => MicPermission::Denied,
        MicrophoneAccess::Undetermined => MicPermission::Undetermined,
    }
}

/// The app macOS asks about the microphone instead of BoardAI ("Visual Studio Code"), or `None`
/// when it asks about BoardAI itself. A process's ancestry doesn't change, so it is worked out
/// once per run. Blocks (it runs `ps`).
pub fn attributed_to() -> Option<String> {
    static ATTRIBUTED: OnceLock<Option<String>> = OnceLock::new();
    ATTRIBUTED
        .get_or_init(|| attributed_app(std::process::id(), parent_of))
        .clone()
}

/// Opens System Settings at Privacy & Security › Microphone. Blocks (it runs `open`).
pub fn open_microphone_settings() -> Result<(), String> {
    let mut command = Command::new("/usr/bin/open");
    command.arg(MICROPHONE_SETTINGS_URL);
    match run_bounded(command) {
        Some((true, _)) => Ok(()),
        Some((false, _)) | None => {
            tracing::warn!("System Settings could not be opened at the microphone pane");
            Err("System Settings didn't open. Open it from the Apple menu instead.".to_owned())
        }
    }
}

/// One process's parent and command, as `ps -o ppid=,comm=` prints them: `  685 /usr/bin/login`.
fn parse_ps_line(output: &str) -> Option<(u32, &str)> {
    let line = output.lines().find(|line| !line.trim().is_empty())?.trim();
    let (ppid, command) = line.split_once(char::is_whitespace)?;
    let command = command.trim();
    (!command.is_empty()).then_some((ppid.parse().ok()?, command))
}

/// The outermost `.app` bundle a command runs from, by name: "Visual Studio Code" for VS Code
/// and for its nested `Code Helper (Plugin).app`.
fn bundle_of(command: &str) -> Option<&str> {
    command
        .split('/')
        .find_map(|component| component.strip_suffix(".app"))
        .filter(|name| !name.is_empty())
}

/// Walks up from `pid` with `parent` (one `ps` answer per process) and names the outermost
/// ancestor inside an `.app` bundle: the app macOS attributes the microphone to. `None` when
/// that is the process's own bundle (BoardAI.app opened from Finder), or no ancestor is one.
fn attributed_app(pid: u32, mut parent: impl FnMut(u32) -> Option<String>) -> Option<String> {
    let mut own: Option<String> = None;
    let mut outermost: Option<String> = None;
    let mut current = pid;
    for step in 0..ATTRIBUTION_ANCESTORS_MAX {
        let output = parent(current)?;
        let (ppid, command) = parse_ps_line(&output)?;
        if let Some(bundle) = bundle_of(command) {
            if step == 0 {
                own = Some(bundle.to_owned());
            }
            outermost = Some(bundle.to_owned());
        }
        // launchd (pid 1) has no parent; the kernel's pid 0 is never asked about.
        if ppid <= 1 || ppid == current {
            break;
        }
        current = ppid;
    }
    outermost.filter(|app| own.as_ref() != Some(app))
}

/// `ps -o ppid=,comm= -p <pid>`.
fn parent_of(pid: u32) -> Option<String> {
    let mut command = Command::new("/bin/ps");
    command.args(["-o", "ppid=,comm=", "-p", &pid.to_string()]);
    match run_bounded(command)? {
        (true, output) => Some(output),
        (false, _) => None,
    }
}

/// Runs a helper process for at most `HELPER_PROCESS_TIMEOUT_MS`, killing it past that: whether
/// it succeeded, and up to `HELPER_OUTPUT_MAX_BYTES` of its output. `None` when it couldn't run
/// or was killed.
fn run_bounded(mut command: Command) -> Option<(bool, String)> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_millis(HELPER_PROCESS_TIMEOUT_MS);
    let exit = loop {
        match child.try_wait() {
            Ok(Some(exit)) => break exit,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(HELPER_POLL_MS));
            }
            Ok(None) | Err(_) => {
                let _killed = child.kill();
                let _reaped = child.wait();
                return None;
            }
        }
    };
    let mut output = String::new();
    if let Some(stdout) = child.stdout.take() {
        stdout
            .take(HELPER_OUTPUT_MAX_BYTES)
            .read_to_string(&mut output)
            .ok()?;
    }
    Some((exit.success(), output))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{ATTRIBUTION_ANCESTORS_MAX, attributed_app, bundle_of, parse_ps_line};

    /// `ps -o ppid=,comm= -p <pid>` answers, by pid, as recorded.
    fn chain(lines: &[(u32, &str)]) -> impl FnMut(u32) -> Option<String> {
        let answers: BTreeMap<u32, String> = lines
            .iter()
            .map(|(pid, line)| (*pid, (*line).to_owned()))
            .collect();
        move |pid| answers.get(&pid).cloned()
    }

    /// `just dev` in Visual Studio Code's terminal: the dev binary, cargo, the tauri CLI, just,
    /// the shell, VS Code's nested helper app, then VS Code itself.
    #[test]
    fn a_dev_build_in_vs_code_is_attributed_to_vs_code() {
        let parent = chain(&[
            (91_200, "91180 /Users/me/boardai/target/debug/boardai"),
            (91_180, "91170 cargo"),
            (91_170, "91160 node"),
            (91_160, "91150 /bin/bash"),
            (91_150, "91140 just"),
            (91_140, " 4521 /bin/zsh"),
            (
                4_521,
                " 4410 /Applications/Visual Studio Code.app/Contents/Frameworks/Code Helper \
                 (Plugin).app/Contents/MacOS/Code Helper (Plugin)",
            ),
            (
                4_410,
                "    1 /Applications/Visual Studio Code.app/Contents/MacOS/Code",
            ),
        ]);
        assert_eq!(
            attributed_app(91_200, parent).as_deref(),
            Some("Visual Studio Code")
        );
    }

    /// Recorded on this machine: a shell in Ghostty, through `login`.
    #[test]
    fn ghostty_and_terminal_are_named() {
        let ghostty = chain(&[
            (73_825, "31303 /bin/zsh"),
            (31_303, "25964 claude"),
            (25_964, "25963 -/bin/zsh"),
            (25_963, "  685 /usr/bin/login"),
            (
                685,
                "    1 /Applications/Ghostty.app/Contents/MacOS/ghostty",
            ),
        ]);
        assert_eq!(attributed_app(73_825, ghostty).as_deref(), Some("Ghostty"));
        let terminal = chain(&[
            (500, "  420 /Users/me/boardai/target/debug/boardai"),
            (420, "  410 -zsh"),
            (410, "  400 login"),
            (
                400,
                "    1 /System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal",
            ),
        ]);
        assert_eq!(attributed_app(500, terminal).as_deref(), Some("Terminal"));
    }

    /// BoardAI.app opened from Finder: macOS asks about BoardAI itself.
    #[test]
    fn a_bundled_app_is_its_own() {
        let bundled = chain(&[(
            700,
            "    1 /Applications/BoardAI.app/Contents/MacOS/BoardAI",
        )]);
        assert_eq!(attributed_app(700, bundled), None);
        let launched_by_launchd = chain(&[(701, "    1 /Users/me/target/debug/boardai")]);
        assert_eq!(attributed_app(701, launched_by_launchd), None);
    }

    #[test]
    fn a_failed_or_endless_walk_names_no_app() {
        assert_eq!(attributed_app(42, chain(&[])), None);
        // A chain that never reaches launchd stops at the bound.
        let mut asked = 0;
        let looping = |pid: u32| {
            asked += 1;
            let command = if pid == 10 {
                "/Users/me/target/debug/boardai"
            } else {
                "/Applications/Loop.app/Contents/MacOS/Loop"
            };
            Some(format!("{} {command}", pid + 1))
        };
        assert_eq!(attributed_app(10, looping).as_deref(), Some("Loop"));
        assert_eq!(asked, ATTRIBUTION_ANCESTORS_MAX);
    }

    #[test]
    fn ps_lines_and_bundles_parse() {
        assert_eq!(
            parse_ps_line("  685 /usr/bin/login\n"),
            Some((685, "/usr/bin/login"))
        );
        assert_eq!(parse_ps_line("1 -/bin/zsh"), Some((1, "-/bin/zsh")));
        assert_eq!(parse_ps_line(""), None);
        assert_eq!(parse_ps_line("abc /bin/zsh"), None);
        assert_eq!(parse_ps_line("  12"), None);
        assert_eq!(
            bundle_of(
                "/Applications/Visual Studio Code.app/Contents/Frameworks/Code Helper.app/Contents/MacOS/Code Helper"
            ),
            Some("Visual Studio Code")
        );
        assert_eq!(bundle_of("/bin/zsh"), None);
        assert_eq!(bundle_of("/Applications/.app/x"), None);
    }
}
