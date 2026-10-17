//! What Claude Code needs from the user's own settings to sign in, and nothing more (finding
//! 10). Confined runs (briefs, spoken research, reviews) load no setting source
//! (`--setting-sources=`), so no user setting can widen what they read: an
//! `additionalDirectories`, an allow rule, a hook, an MCP server. Signing in mostly needs no
//! setting at all (Claude Code's OAuth sign-in is in the keychain, an API key in the
//! environment), but an `apiKeyHelper`, a cloud provider's refresh command or its selection in
//! `env` is one: those, and only those, are read once where the harness is set up and handed
//! back in the run's own `--settings`.
//!
//! An `env` value that holds a secret (a key, a token, a password) is never forwarded: it would
//! sit on the command line. A user who keeps one in `settings.json` gets confined runs without
//! it, and Claude Code says it isn't signed in.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::limits::{CLAUDE_SETTINGS_READ_MAX_BYTES, CLAUDE_SIGN_IN_VALUE_MAX_BYTES};

/// Settings keys Claude Code signs in with: commands the user configured, and the login method.
const SIGN_IN_KEYS: &[&str] = &[
    "apiKeyHelper",
    "awsAuthRefresh",
    "awsCredentialExport",
    "gcpAuthRefresh",
    "forceLoginMethod",
    "forceLoginOrgUUID",
];

/// `env` names that choose and locate the provider; none of them holds a secret.
const SIGN_IN_ENV: &[&str] = &[
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_BEDROCK_BASE_URL",
    "ANTHROPIC_VERTEX_BASE_URL",
    "ANTHROPIC_FOUNDRY_BASE_URL",
    "ANTHROPIC_VERTEX_PROJECT_ID",
    "ANTHROPIC_FOUNDRY_RESOURCE",
    "CLOUD_ML_REGION",
    "AWS_REGION",
    "AWS_PROFILE",
];

/// The sign-in settings to hand a confined run; empty for most users.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClaudeSignIn {
    settings: Map<String, Value>,
}

impl ClaudeSignIn {
    /// Reads the user's `settings.json` (`$CLAUDE_CONFIG_DIR`, else `~/.claude`), bounded by
    /// `CLAUDE_SETTINGS_READ_MAX_BYTES`. Missing, too large or unreadable: nothing is forwarded,
    /// and a run that needed it says it isn't signed in.
    #[must_use]
    pub fn read(home: &Path) -> Self {
        let folder = std::env::var_os("CLAUDE_CONFIG_DIR")
            .map_or_else(|| home.join(".claude"), PathBuf::from);
        let path = folder.join("settings.json");
        let Ok(metadata) = std::fs::metadata(&path) else {
            return Self::default();
        };
        let bound = u64::try_from(CLAUDE_SETTINGS_READ_MAX_BYTES).unwrap_or(u64::MAX);
        if metadata.len() > bound {
            tracing::warn!("Claude Code's settings are too large to read; none are forwarded");
            return Self::default();
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::from_settings(&text),
            Err(error) => {
                tracing::warn!(%error, "Claude Code's settings couldn't be read");
                Self::default()
            }
        }
    }

    /// The sign-in entries of a `settings.json`'s text; nothing when it isn't a JSON object.
    #[must_use]
    pub fn from_settings(text: &str) -> Self {
        let Ok(Value::Object(user)) = serde_json::from_str::<Value>(text) else {
            return Self::default();
        };
        let short = |value: &Value| {
            value
                .as_str()
                .filter(|text| text.len() <= CLAUDE_SIGN_IN_VALUE_MAX_BYTES)
                .map(|text| Value::String(text.to_owned()))
        };
        let mut settings = Map::new();
        for key in SIGN_IN_KEYS {
            if let Some(value) = user.get(*key).and_then(short) {
                settings.insert((*key).to_owned(), value);
            }
        }
        let env: Map<String, Value> = user
            .get("env")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .filter(|(name, _)| SIGN_IN_ENV.contains(&name.as_str()))
            .filter_map(|(name, value)| Some((name.clone(), short(value)?)))
            .collect();
        if !env.is_empty() {
            settings.insert("env".to_owned(), Value::Object(env));
        }
        Self { settings }
    }

    /// Adds the sign-in entries to a run's settings, beside its permission rules.
    pub(crate) fn add_to(&self, settings: &mut Map<String, Value>) {
        for (key, value) in &self.settings {
            settings.insert(key.clone(), value.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, json};

    use super::ClaudeSignIn;

    /// Only signing in is forwarded: never a directory, a rule, a hook, an MCP server, or a
    /// secret held in `env`.
    #[test]
    fn only_what_signs_in_is_forwarded() {
        let user = json!({
            "apiKeyHelper": "/usr/local/bin/anthropic-key",
            "permissions": {
                "additionalDirectories": ["/Users/me"],
                "allow": ["Read(//Users/me/**)"],
                "defaultMode": "bypassPermissions",
            },
            "hooks": {"PreToolUse": [{"matcher": "Read", "hooks": [{"type": "command", "command": "approve"}]}]},
            "enabledMcpjsonServers": ["files"],
            "env": {
                "CLAUDE_CODE_USE_BEDROCK": "1",
                "AWS_PROFILE": "work",
                "AWS_SECRET_ACCESS_KEY": concat!("wJalr", "XUtnFEMI"),
                "ANTHROPIC_API_KEY": concat!("sk-", "ant-test"),
                "CLAUDE_CODE_ADDITIONAL_DIRECTORIES": "/Users/me",
            },
            "model": "opus",
        });
        let sign_in = ClaudeSignIn::from_settings(&user.to_string());
        let mut settings = Map::new();
        sign_in.add_to(&mut settings);
        assert_eq!(
            serde_json::Value::Object(settings),
            json!({
                "apiKeyHelper": "/usr/local/bin/anthropic-key",
                "env": {"CLAUDE_CODE_USE_BEDROCK": "1", "AWS_PROFILE": "work"},
            })
        );
        assert_eq!(
            ClaudeSignIn::from_settings("// not json"),
            ClaudeSignIn::default()
        );
        assert_eq!(ClaudeSignIn::from_settings("[]"), ClaudeSignIn::default());
    }
}
