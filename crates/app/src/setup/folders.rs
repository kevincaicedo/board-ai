//! Code folders and the vault on the settings page. A folder is added only through the macOS
//! folder chooser, which the app shows itself (`osascript`'s `choose folder`), so a path never
//! comes from the UI: the screen asks to add one and learns the result from the status. The
//! chooser is a separate process rather than a dialog plugin (no new dependency, law 9).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use boardai_runtime::settings::{Settings, SettingsEdit};
use boardai_runtime::tools::{TooBroad, too_broad};

use super::SetupPaths;
use crate::helper::{Finished, HELPER_PROCESS_TIMEOUT_MS, run_bounded};

/// How long the folder chooser may stay open. Past it the chooser is closed and the card says
/// it timed out; nothing is added.
pub const FOLDER_DIALOG_WAIT_MS: u64 = 300_000;

/// Code folders kept in `settings.toml`. Past it adding one is refused until one is removed,
/// which keeps the file well under its size bound.
pub const CODE_FOLDERS_MAX: usize = 32;

/// AppleScript's "User canceled." error.
const USER_CANCELED: &str = "(-128)";

const CHOOSE_FOLDER: &str =
    "POSIX path of (choose folder with prompt \"Choose a code folder BoardAI may read\")";

/// The Sources tray's chooser (D-184): a folder or a file to share.
pub const CHOOSE_SOURCE_FOLDER: &str =
    "POSIX path of (choose folder with prompt \"Choose a folder for BoardAI to read\")";
pub const CHOOSE_SOURCE_FILE: &str =
    "POSIX path of (choose file with prompt \"Choose a file for BoardAI to read\")";

/// What the chooser answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Chosen {
    Folder(PathBuf),
    /// Closed without a choice: not a failure, nothing changes.
    Cancelled,
}

/// Shows the macOS folder chooser and waits for it. Blocks for up to `FOLDER_DIALOG_WAIT_MS`.
pub fn choose_folder() -> Result<Chosen, String> {
    choose(CHOOSE_FOLDER)
}

/// Shows a macOS chooser (`choose folder` or `choose file`, the script given) and waits for it,
/// bounded the same way. The path comes from the system, never from the UI (D-173).
pub fn choose(script: &str) -> Result<Chosen, String> {
    let mut command = Command::new("/usr/bin/osascript");
    command.args(["-e", script]);
    let finished = run_bounded(command, Duration::from_millis(FOLDER_DIALOG_WAIT_MS));
    parse_chosen(finished.as_ref())
}

/// The chooser's answer, parsed where it enters: one absolute path to a folder, without control
/// characters, or a cancel.
pub fn parse_chosen(finished: Option<&Finished>) -> Result<Chosen, String> {
    let Some(finished) = finished else {
        return Err("The folder chooser didn't answer in time. Try again.".to_owned());
    };
    if !finished.success {
        if finished.stderr.contains(USER_CANCELED) {
            return Ok(Chosen::Cancelled);
        }
        tracing::warn!("the folder chooser failed");
        return Err("The folder chooser didn't open. Try again.".to_owned());
    }
    let text = finished
        .stdout
        .strip_suffix('\n')
        .unwrap_or(&finished.stdout);
    let trimmed = if text.len() > 1 {
        text.trim_end_matches('/')
    } else {
        text
    };
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) || !trimmed.starts_with('/') {
        return Err("BoardAI can't use that folder's name. Pick another folder.".to_owned());
    }
    Ok(Chosen::Folder(PathBuf::from(trimmed)))
}

/// A path as `settings.toml` writes it and the screen shows it: `~/…` under the home folder.
pub fn as_written(path: &Path, home: &Path) -> String {
    if path == home {
        return "~".to_owned();
    }
    match path.strip_prefix(home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// The settings change that adds `folder`, or why it can't be added.
pub fn add_folder(current: &Settings, folder: &Path, home: &Path) -> Result<SettingsEdit, String> {
    if current.code_roots.iter().any(|root| root == folder) {
        return Err("That folder is already in the list.".to_owned());
    }
    if current.code_roots.len() >= CODE_FOLDERS_MAX {
        return Err(format!(
            "BoardAI keeps at most {CODE_FOLDERS_MAX} code folders. Remove one first."
        ));
    }
    if !folder.is_dir() {
        return Err("That folder isn't there any more. Pick another.".to_owned());
    }
    match too_broad(folder, home) {
        Some(TooBroad::HomeOrAbove) => Err(
            "That holds every file of yours, keys included. Pick the folder your code is in."
                .to_owned(),
        ),
        Some(TooBroad::AppData) => Err(
            "That's where apps keep their data and keychains. Pick the folder your code is in."
                .to_owned(),
        ),
        None => Ok(SettingsEdit::AddCodeRoot(as_written(folder, home))),
    }
}

/// The vault "Open the vault" opens, from `settings.toml`. A file that doesn't parse opens
/// nothing (not the default vault, which isn't the one configured): its error says which field
/// and never quotes a value (D-025).
pub fn configured_vault(paths: &SetupPaths) -> Result<PathBuf, String> {
    Settings::load(&paths.settings, &paths.home)
        .map(|settings| settings.vault)
        .map_err(|error| format!("settings.toml can't be used ({error}). Fix it, then try again."))
}

/// "Open the vault": the vault folder in Finder. Blocks (it runs `open`).
pub fn open_vault(vault: &Path) -> Result<(), String> {
    let vault = vault_to_open(vault)?;
    let mut command = Command::new("/usr/bin/open");
    command.arg(vault);
    match run_bounded(command, Duration::from_millis(HELPER_PROCESS_TIMEOUT_MS)) {
        Some(finished) if finished.success => Ok(()),
        Some(_) | None => {
            tracing::warn!("Finder could not be opened at the vault");
            Err("Finder didn't open the vault. Open it from Finder instead.".to_owned())
        }
    }
}

/// The vault as `open` may take it: an absolute path to a folder that Finder shows rather than
/// opens. A relative value, or one starting with `-`, from a hand-edited `settings.toml` would
/// reach `open` as a path relative to the app's folder or as an option; a folder whose name has
/// an extension may be a package (`Safari.app`, `.photoslibrary`, `.pkg`), which `open`
/// launches. Neither ever runs: no list of package kinds is complete, so any extension is
/// refused (the default vault, `~/BoardAI`, has none).
fn vault_to_open(vault: &Path) -> Result<&Path, String> {
    if !vault.is_absolute() {
        return Err(
            "settings.toml's vault isn't an absolute path. Fix it there, then try again."
                .to_owned(),
        );
    }
    if vault.extension().is_some() {
        return Err(
            "The vault's name ends like an app or a package, so Finder would open it instead of \
             showing it. Rename it in settings.toml."
                .to_owned(),
        );
    }
    if !vault.is_dir() {
        return Err(
            "The vault isn't there yet. BoardAI makes it when a session starts.".to_owned(),
        );
    }
    Ok(vault)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use boardai_runtime::settings::{Settings, SettingsEdit};

    use super::{
        CODE_FOLDERS_MAX, Chosen, add_folder, as_written, configured_vault, open_vault,
        parse_chosen, vault_to_open,
    };
    use crate::helper::Finished;
    use crate::setup::SetupPaths;

    fn answered(success: bool, stdout: &str, stderr: &str) -> Finished {
        Finished {
            success,
            stdout: stdout.to_owned(),
            stderr: stderr.to_owned(),
        }
    }

    #[test]
    fn the_chooser_answers_a_folder_a_cancel_or_a_failure() {
        assert_eq!(
            parse_chosen(Some(&answered(true, "/Users/me/code/boardai/\n", ""))),
            Ok(Chosen::Folder(PathBuf::from("/Users/me/code/boardai")))
        );
        assert_eq!(
            parse_chosen(Some(&answered(true, "/\n", ""))),
            Ok(Chosen::Folder(PathBuf::from("/")))
        );
        let cancelled = answered(false, "", "execution error: User canceled. (-128)\n");
        assert_eq!(parse_chosen(Some(&cancelled)), Ok(Chosen::Cancelled));
        assert!(parse_chosen(Some(&answered(false, "", "syntax error"))).is_err());
        assert!(
            parse_chosen(None).is_err(),
            "a chooser past its bound adds nothing"
        );
        for bad in ["", "\n", "relative/path\n", "/Users/me/two\nlines\n"] {
            assert!(
                parse_chosen(Some(&answered(true, bad, ""))).is_err(),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn folders_are_written_under_home_with_a_tilde() {
        let home = Path::new("/Users/me");
        assert_eq!(as_written(Path::new("/Users/me/code"), home), "~/code");
        assert_eq!(as_written(home, home), "~");
        assert_eq!(as_written(Path::new("/opt/src"), home), "/opt/src");
        assert_eq!(as_written(Path::new("/Users/meg/x"), home), "/Users/meg/x");
    }

    #[test]
    fn a_folder_is_added_once_while_there_is_room() {
        let scratch = std::env::temp_dir();
        let home = Path::new("/Users/me");
        let mut current = Settings::defaults(home);
        assert_eq!(
            add_folder(&current, &scratch, home),
            Ok(SettingsEdit::AddCodeRoot(as_written(&scratch, home)))
        );
        current.code_roots.push(scratch.clone());
        assert!(
            add_folder(&current, &scratch, home).is_err(),
            "already listed"
        );
        let gone = scratch.join("boardai-no-such-folder-for-settings");
        current.code_roots.clear();
        assert!(add_folder(&current, &gone, home).is_err());
        current.code_roots = (0..CODE_FOLDERS_MAX)
            .map(|index| PathBuf::from(format!("/src/{index}")))
            .collect();
        assert!(add_folder(&current, &scratch, home).is_err(), "full");
    }

    /// Review 2, finding 10: `/` or `~` as a code folder put `~/.ssh` in `read_file`'s reach.
    #[test]
    fn the_disk_the_home_folder_and_app_data_are_never_code_folders() {
        let scratch = std::env::temp_dir().join(format!("boardai-broad-{}", std::process::id()));
        let home = scratch.join("Users/me");
        for folder in ["code/app", "Library/Application Support/x", "Library"] {
            std::fs::create_dir_all(home.join(folder)).expect("a folder");
        }
        let current = Settings::defaults(&home);
        for broad in [
            PathBuf::from("/"),
            scratch.join("Users"),
            home.clone(),
            home.join("Library"),
            home.join("Library/Application Support/x"),
        ] {
            let refused = add_folder(&current, &broad, &home);
            assert!(
                refused
                    .as_ref()
                    .is_err_and(|words| words.contains("Pick the folder")),
                "{} was accepted: {refused:?}",
                broad.display()
            );
        }
        assert_eq!(
            add_folder(&current, &home.join("code/app"), &home),
            Ok(SettingsEdit::AddCodeRoot("~/code/app".to_owned()))
        );
        let _ = std::fs::remove_dir_all(&scratch);
    }

    /// Review 2, finding 10: `open` on a bundle launches it ("Open the vault" on `Safari.app`
    /// would start Safari); a folder whose name has an extension is refused before `open`.
    #[test]
    fn a_vault_that_finder_would_open_as_a_package_never_reaches_open() {
        let scratch = std::env::temp_dir().join(format!("boardai-bundle-{}", std::process::id()));
        for package in ["Fake.app", "Photos Library.photoslibrary", "Tool.APP"] {
            let folder = scratch.join(package);
            std::fs::create_dir_all(&folder).expect("a folder");
            assert!(
                vault_to_open(&folder).is_err_and(|words| words.contains("package")),
                "{package} would reach open"
            );
        }
        let plain = scratch.join(".boardai-vault");
        std::fs::create_dir_all(&plain).expect("a folder");
        assert_eq!(vault_to_open(&plain), Ok(plain.as_path()));
        let _ = std::fs::remove_dir_all(&scratch);
    }

    /// A `settings.toml` that doesn't parse used to open the default vault; it is refused with
    /// the parse error, which names the field and never quotes a value (D-025).
    #[test]
    fn a_settings_file_that_does_not_parse_opens_no_vault() {
        let scratch =
            std::env::temp_dir().join(format!("boardai-vault-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&scratch).expect("a folder");
        let paths = SetupPaths {
            settings: scratch.join("settings.toml"),
            home: scratch.join("home"),
            models: scratch.join("models"),
        };
        std::fs::write(&paths.settings, "vault = 42\n").expect("written");
        let refused = configured_vault(&paths);
        assert!(
            refused
                .as_ref()
                .is_err_and(|words| words.contains("settings.toml")),
            "{refused:?}"
        );
        std::fs::write(&paths.settings, "vault = \"~/Notes\"\n").expect("written");
        assert_eq!(configured_vault(&paths), Ok(paths.home.join("Notes")));
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn a_missing_vault_is_said_without_running_finder() {
        let missing = std::env::temp_dir().join("boardai-no-vault-here");
        assert!(open_vault(&missing).is_err());
    }

    #[test]
    fn only_an_absolute_vault_ever_reaches_open() {
        for unsafe_value in [
            "-a",
            "--args",
            "-R",
            "BoardAI",
            "./BoardAI",
            "~/BoardAI",
            "",
        ] {
            let refused = vault_to_open(Path::new(unsafe_value));
            assert!(
                refused.is_err_and(|words| words.contains("absolute path")),
                "{unsafe_value:?} would reach open"
            );
        }
        let folder = std::env::temp_dir();
        assert!(folder.is_absolute());
        assert_eq!(vault_to_open(&folder), Ok(folder.as_path()));
    }
}
