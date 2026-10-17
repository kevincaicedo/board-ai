//! A research brief's harness reads only inside its reach (D-226): the shared source's
//! enclosing git repository (`sources::reach`). The prompt says so to both harnesses; only
//! Claude Code can enforce it, with its own permission rules:
//!
//! - `--tools Read,Grep,Glob`: no other built-in tool exists in the session (no Bash, no edits,
//!   no web, no subagents), and `--strict-mcp-config` loads no MCP server.
//! - `--permission-mode dontAsk`: whatever no rule allows is denied, never asked.
//! - `--setting-sources user`: the shared repository's own `.claude/` settings (its hooks,
//!   allow rules, MCP servers) are not loaded; it is untrusted input.
//! - `--settings` with [`claude_rules`]: `Read` allowed on the reach's path pattern only (Claude
//!   Code applies `Read` rules to Grep and Glob), every other tool and the credential paths
//!   denied, the reach as the one extra working directory, and
//!   `blockReadsOutsideWorkingDirectories`, which refuses reads outside the working directories
//!   in every mode, whatever allow rule a user's own settings add.
//!
//! pi's RPC mode has no permission rules: it gets the read-only tool set
//! (`--tools read,grep,find,ls`), the source folder as its working directory, and the prompt.

use std::path::Path;

use serde_json::json;

use crate::tools::CREDENTIAL_PATHS;

/// Claude Code tools denied by name: removed from the session even if a setting allows one.
const DENIED_TOOLS: &[&str] = &[
    "Bash",
    "Edit",
    "Write",
    "NotebookEdit",
    "WebFetch",
    "WebSearch",
    "Agent",
    "mcp__*",
];

/// Files under the reach never read, besides [`CREDENTIAL_PATHS`]: keys live there too.
const DENIED_FILES: &[&str] = &[".env", ".env.*", "*.pem", "*.key", "id_rsa*", "id_ed25519*"];

/// The line appended to a brief's system prompt: where the harness may read, as a path from
/// its working folder (no absolute path goes to the model).
#[must_use]
pub fn reach_line(folder: &Path, reach: &Path) -> String {
    let depth = folder
        .strip_prefix(reach)
        .map_or(0, |inside| inside.components().count());
    if depth == 0 {
        return "Read only inside the working folder; never outside it.".to_owned();
    }
    let up = vec![".."; depth].join("/");
    format!(
        "Read only inside the repository this folder is in ({up} from here) and never outside \
         it; cite files outside this folder by their path from here, such as {up}/README.md:1."
    )
}

/// Claude Code's `--settings` for a brief: the permission rules that hold it to `reach`
/// (canonical). `folder` is the working directory; the reach is added as a working directory
/// when it is wider.
#[must_use]
pub fn claude_rules(folder: &Path, reach: &Path) -> String {
    let anchor = pattern_anchor(reach);
    let mut deny: Vec<String> = DENIED_TOOLS.iter().map(|tool| (*tool).to_owned()).collect();
    for parts in CREDENTIAL_PATHS {
        let joined = parts.join("/");
        deny.push(format!("Read({anchor}/**/{joined})"));
        deny.push(format!("Read({anchor}/**/{joined}/**)"));
    }
    for file in DENIED_FILES {
        deny.push(format!("Read({anchor}/**/{file})"));
    }
    let mut permissions = json!({
        "allow": [format!("Read({anchor}/**)")],
        "deny": deny,
        "blockReadsOutsideWorkingDirectories": true,
        "disableBypassPermissionsMode": "disable",
    });
    if folder != reach {
        permissions["additionalDirectories"] = json!([reach.to_string_lossy()]);
    }
    json!({ "permissions": permissions }).to_string()
}

/// `//` and the absolute path, with gitignore's pattern characters escaped so the rule names
/// the folder as spelled.
fn pattern_anchor(path: &Path) -> String {
    let mut anchor = String::from("/");
    for character in path.to_string_lossy().chars() {
        if matches!(character, '\\' | '*' | '?' | '[' | ']') {
            anchor.push('\\');
        }
        anchor.push(character);
    }
    anchor
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::Value;

    use super::{claude_rules, reach_line};

    #[test]
    fn the_rules_allow_reading_the_reach_and_deny_the_rest() {
        let rules: Value = serde_json::from_str(&claude_rules(
            Path::new("/r/db/crates/a"),
            Path::new("/r/db"),
        ))
        .expect("json");
        let permissions = &rules["permissions"];
        assert_eq!(permissions["allow"], serde_json::json!(["Read(//r/db/**)"]));
        let deny: Vec<&str> = permissions["deny"]
            .as_array()
            .expect("a list")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        for denied in [
            "Bash",
            "Edit",
            "WebFetch",
            "mcp__*",
            "Read(//r/db/**/.ssh/**)",
            "Read(//r/db/**/.env)",
        ] {
            assert!(deny.contains(&denied), "{denied} in {deny:?}");
        }
        assert_eq!(permissions["blockReadsOutsideWorkingDirectories"], true);
        assert_eq!(permissions["additionalDirectories"][0], "/r/db");
        let alone: Value =
            serde_json::from_str(&claude_rules(Path::new("/r/db"), Path::new("/r/db")))
                .expect("json");
        assert!(alone["permissions"]["additionalDirectories"].is_null());
    }

    #[test]
    fn pattern_characters_in_the_path_are_escaped() {
        let rules = claude_rules(Path::new("/r/a[1]*"), Path::new("/r/a[1]*"));
        assert!(rules.contains(r#""Read(//r/a\\[1\\]\\*/**)""#), "{rules}");
    }

    #[test]
    fn the_prompt_names_the_reach_from_the_working_folder() {
        assert_eq!(
            reach_line(Path::new("/r/db"), Path::new("/r/db")),
            "Read only inside the working folder; never outside it."
        );
        let line = reach_line(Path::new("/r/db/crates/a"), Path::new("/r/db"));
        assert!(line.contains("(../.. from here)"), "{line}");
        assert!(!line.contains("/r/db"), "no absolute path: {line}");
    }
}
