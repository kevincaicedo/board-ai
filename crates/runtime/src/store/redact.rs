//! Secret redaction at the store boundary (LLD §2.8, invariant I8, law 10).
//!
//! Every byte the store writes (event log lines, vault files, index rows) is a [`Redacted`],
//! and a `Redacted` can only come out of [`redact_text`] or [`redact_json_text`]. The writers
//! take nothing else, so an unredacted write path does not compile.
//!
//! The rules are a table, applied in order: specific key formats first, then headers, then
//! `password=`-style assignments. Each rule has a planted-secret test, and ordinary code and
//! prose have false-positive tests. A match is replaced by `[redacted:<rule>]`; an assignment
//! keeps its name and loses only its value.

use std::ops::Range;
use std::sync::LazyLock;

use regex::{Regex, RegexSet};

/// Text that has been through redaction. The only way to make one is to redact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Redacted {
    text: String,
    count: usize,
}

impl Redacted {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub fn into_string(self) -> String {
        self.text
    }

    /// How many secrets were replaced.
    #[must_use]
    pub const fn count(&self) -> usize {
        self.count
    }

    /// The text as one bounded item line ([`one_line`]). Cutting comes after redaction, never
    /// before: a secret cut short of the length its rule matches would slip through.
    ///
    /// [`one_line`]: crate::store::vault::markdown::one_line
    #[must_use]
    pub(crate) fn one_line(&self, max_bytes: usize) -> Self {
        Self {
            text: crate::store::vault::markdown::one_line(&self.text, max_bytes),
            count: self.count,
        }
    }

    /// The text cut to at most `max_bytes` (plus `…`), after redaction for the same reason.
    #[must_use]
    pub(crate) fn cut(&self, max_bytes: usize) -> Self {
        Self {
            text: crate::store::vault::markdown::cut_bytes(&self.text, max_bytes),
            count: self.count,
        }
    }
}

/// When a candidate match counts as a secret.
#[derive(Clone, Copy, Debug)]
enum Check {
    Always,
    /// Letters of both cases and a digit: random keys have them, kebab-case names don't.
    MixedCaseWithDigit,
    HasDigit,
    /// A digit, or both letter cases (base64 credentials): not an English word.
    LooksRandom,
    /// A `password=` value: see [`assignment_is_secret`].
    Assignment,
}

struct Rule {
    name: &'static str,
    /// The secret is the `secret`, `quoted`, `single` or `bare` group when the pattern has
    /// one (the name before it stays readable), otherwise the whole match.
    pattern: &'static str,
    check: Check,
}

const RULES: &[Rule] = &[
    Rule {
        name: "private-key",
        pattern: r"(?s)-----BEGIN[A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----.*?(?:-----END[A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----|\z)",
        check: Check::Always,
    },
    Rule {
        name: "anthropic-key",
        pattern: r"\bsk-ant-[A-Za-z0-9_\-]{16,}",
        check: Check::Always,
    },
    Rule {
        name: "openrouter-key",
        pattern: r"\bsk-or-[A-Za-z0-9_\-]{16,}",
        check: Check::Always,
    },
    Rule {
        name: "openai-key",
        pattern: r"\bsk-(?:proj-|svcacct-|admin-)?[A-Za-z0-9_\-]{20,}",
        check: Check::MixedCaseWithDigit,
    },
    Rule {
        name: "stripe-key",
        pattern: r"\b[rs]k_(?:live|test)_[A-Za-z0-9]{16,}",
        check: Check::Always,
    },
    Rule {
        name: "elevenlabs-key",
        // Real keys are 48 hex characters; 20 still catches a truncated or older key, and the
        // digit check keeps `sk_` identifiers out.
        pattern: r"\bsk_[A-Za-z0-9]{20,}",
        check: Check::HasDigit,
    },
    Rule {
        name: "github-token",
        pattern: r"\b(?:gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{30,})",
        check: Check::Always,
    },
    Rule {
        name: "aws-access-key-id",
        pattern: r"\b(?:AKIA|ASIA|ABIA|ACCA)[A-Z0-9]{16}\b",
        check: Check::Always,
    },
    Rule {
        name: "aws-secret-key",
        pattern: r#"(?i)\baws_?secret_?(?:access_?)?key["']?\s*[:=]\s*["']?(?P<secret>[A-Za-z0-9/+]{40})"#,
        check: Check::Always,
    },
    Rule {
        name: "google-api-key",
        pattern: r"\bAIza[0-9A-Za-z_\-]{35}",
        check: Check::Always,
    },
    Rule {
        name: "slack-token",
        pattern: r"\bxox[abposr]-[A-Za-z0-9\-]{10,}",
        check: Check::Always,
    },
    Rule {
        name: "jwt",
        pattern: r"\beyJ[A-Za-z0-9_\-]{8,}\.eyJ[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]*",
        check: Check::Always,
    },
    Rule {
        name: "url-password",
        pattern: r"\b[A-Za-z][A-Za-z0-9+.\-]*://[^/\s:@]+:(?P<secret>[^/\s@]+)@",
        check: Check::Always,
    },
    Rule {
        name: "authorization",
        pattern: r#"(?i)\bauthorization["']?\s*[:=]\s*["']?(?:(?:bearer|basic|token|bot)\s+)?(?P<secret>[A-Za-z0-9\-._~+/]{8,}=*)"#,
        check: Check::LooksRandom,
    },
    Rule {
        name: "bearer-token",
        pattern: r"(?i)\bbearer\s+(?P<secret>[A-Za-z0-9\-._~+/]{16,}=*)",
        check: Check::LooksRandom,
    },
    Rule {
        name: "api-key-header",
        pattern: r#"(?i)\b(?:x-api-key|xi-api-key|api-key|x-auth-token)["']?\s*[:=]\s*["']?(?P<secret>[A-Za-z0-9\-._~+/]{12,})"#,
        check: Check::LooksRandom,
    },
    Rule {
        name: "assigned-secret",
        pattern: r#"(?i)\b[a-z0-9_]*?(?:password|passwd|passphrase|pwd|secret|token|api_?key|access_?key|private_?key|auth_?key)\b["']?\s*(?::=|=|:)\s*(?:"(?P<quoted>[^"\n]{4,})"|'(?P<single>[^'\n]{4,})'|(?P<bare>[^\s"'`,;&()\[\]{}<>]{6,}))"#,
        check: Check::Assignment,
    },
];

const SECRET_GROUPS: [&str; 4] = ["secret", "quoted", "single", "bare"];
const MARKER_PREFIX: &str = "[redacted:";

struct Compiled {
    /// One pass over the text tells which rules can match; most text matches none.
    set: RegexSet,
    rules: Vec<Regex>,
}

#[allow(
    clippy::expect_used,
    reason = "the patterns are constants and `every_rule_compiles` builds them in a test; \
              writing unredacted text instead would break law 10"
)]
static COMPILED: LazyLock<Compiled> = LazyLock::new(|| Compiled {
    set: RegexSet::new(RULES.iter().map(|rule| rule.pattern)).expect("redaction rules compile"),
    rules: RULES
        .iter()
        .map(|rule| Regex::new(rule.pattern).expect("redaction rule compiles"))
        .collect(),
});

/// Redacts secrets in plain text or Markdown.
#[must_use]
pub fn redact_text(text: &str) -> Redacted {
    match redact_str(text) {
        Some((text, count)) => Redacted { text, count },
        None => Redacted {
            text: text.to_owned(),
            count: 0,
        },
    }
}

/// Redacts secrets inside the string literals of JSON text, leaving everything else byte for
/// byte (so field order and formatting are unchanged, and the result is still the same JSON
/// shape). Text that is not JSON is still redacted, string by string or as a whole.
#[must_use]
pub fn redact_json_text(json: &str) -> Redacted {
    let bytes = json.as_bytes();
    let mut out = String::with_capacity(json.len());
    let mut count = 0;
    let mut cursor = 0;
    // A code card holds a file one line per string, so a key file's body lines are secret only
    // by their place after `BEGIN … PRIVATE KEY`: that state carries across strings.
    let mut in_key_block = false;
    // Text between string literals is redacted too: in valid JSON it holds only numbers and
    // punctuation, but text that isn't JSON must not slip through unredacted.
    loop {
        let rest = bytes.get(cursor..).unwrap_or_default();
        let Some(open) = rest
            .iter()
            .position(|&byte| byte == b'"')
            .map(|at| cursor + at)
        else {
            count += push_redacted(&mut out, json.get(cursor..).unwrap_or_default());
            break;
        };
        count += push_redacted(&mut out, json.get(cursor..open).unwrap_or_default());
        let Some((close, has_escape)) = string_end(bytes, open + 1) else {
            count += push_redacted(&mut out, json.get(open..).unwrap_or_default());
            break;
        };
        match redact_literal(json, open, close, has_escape, &mut in_key_block) {
            Some((literal, found)) => {
                out.push_str(&literal);
                count += found;
            }
            None => out.push_str(json.get(open..=close).unwrap_or_default()),
        }
        cursor = close + 1;
    }
    Redacted { text: out, count }
}

/// Appends `segment` redacted as plain text; returns the number of secrets replaced.
fn push_redacted(out: &mut String, segment: &str) -> usize {
    match redact_str(segment) {
        Some((redacted, found)) => {
            out.push_str(&redacted);
            found
        }
        None => {
            out.push_str(segment);
            0
        }
    }
}

/// The index of the closing quote of a string whose content starts at `begin`, and whether the
/// string has escapes. `None` when the input ends first.
fn string_end(bytes: &[u8], begin: usize) -> Option<(usize, bool)> {
    let mut index = begin;
    let mut has_escape = false;
    while let Some(&byte) = bytes.get(index) {
        match byte {
            b'\\' => {
                has_escape = true;
                index += 2;
            }
            b'"' => return Some((index, has_escape)),
            _ => index += 1,
        }
    }
    None
}

/// The replacement for the string literal `json[open..=close]`, when it holds a secret.
fn redact_literal(
    json: &str,
    open: usize,
    close: usize,
    has_escape: bool,
    in_key_block: &mut bool,
) -> Option<(String, usize)> {
    let literal = json.get(open..=close)?;
    let decoded: String = if has_escape {
        match serde_json::from_str::<String>(literal) {
            Ok(decoded) => decoded,
            // Not a valid JSON string (a bad escape): redact the raw text between the quotes.
            Err(_) => literal.get(1..literal.len() - 1)?.to_owned(),
        }
    } else {
        literal.get(1..literal.len() - 1)?.to_owned()
    };
    if *in_key_block {
        *in_key_block = !(decoded.contains("-----END") && decoded.contains("PRIVATE KEY"));
        let marker = format!("{MARKER_PREFIX}private-key]");
        return Some((serde_json::to_string(&marker).ok()?, 1));
    }
    *in_key_block = opens_key_block(&decoded);
    let (redacted, count) = redact_str(&decoded)?;
    let encoded = serde_json::to_string(&redacted).ok()?;
    Some((encoded, count))
}

/// Whether `text` starts a private key block it doesn't finish.
fn opens_key_block(text: &str) -> bool {
    let Some(begin) = text.rfind("-----BEGIN") else {
        return false;
    };
    let after = &text[begin..];
    after.contains("PRIVATE KEY") && !after.contains("-----END")
}

/// The redacted text and the number of secrets replaced, or `None` when there were none.
fn redact_str(text: &str) -> Option<(String, usize)> {
    let compiled = &*COMPILED;
    let matched = compiled.set.matches(text);
    if !matched.matched_any() {
        return None;
    }
    let mut current = text.to_owned();
    let mut total = 0;
    for rule_index in matched.iter() {
        let (Some(rule), Some(regex)) = (RULES.get(rule_index), compiled.rules.get(rule_index))
        else {
            continue;
        };
        if let Some((next, count)) = apply_rule(rule, regex, &current) {
            current = next;
            total += count;
        }
    }
    (total > 0).then_some((current, total))
}

fn apply_rule(rule: &Rule, regex: &Regex, text: &str) -> Option<(String, usize)> {
    let mut out = String::new();
    let mut copied_to = 0;
    let mut count = 0;
    for captures in regex.captures_iter(text) {
        let Some(whole) = captures.get(0) else {
            continue;
        };
        let (group, target) = SECRET_GROUPS
            .iter()
            .find_map(|name| captures.name(name).map(|found| (*name, found)))
            .unwrap_or(("", whole));
        let range = target.range();
        // A value that is already a marker, or overlaps one, stays as it is.
        if target.as_str().contains(MARKER_PREFIX) || range.start < copied_to {
            continue;
        }
        if !keeps(rule.check, group, text, range.clone()) {
            continue;
        }
        out.push_str(text.get(copied_to..range.start).unwrap_or_default());
        out.push_str(MARKER_PREFIX);
        out.push_str(rule.name);
        out.push(']');
        copied_to = range.end;
        count += 1;
    }
    if count == 0 {
        return None;
    }
    out.push_str(text.get(copied_to..).unwrap_or_default());
    Some((out, count))
}

fn keeps(check: Check, group: &str, text: &str, range: Range<usize>) -> bool {
    let secret = text.get(range.clone()).unwrap_or_default();
    let has_digit = secret.bytes().any(|byte| byte.is_ascii_digit());
    let has_upper = secret.bytes().any(|byte| byte.is_ascii_uppercase());
    let has_lower = secret.bytes().any(|byte| byte.is_ascii_lowercase());
    match check {
        Check::Always => true,
        Check::MixedCaseWithDigit => has_digit && has_upper && has_lower,
        Check::HasDigit => has_digit,
        Check::LooksRandom => has_digit || (has_upper && has_lower),
        Check::Assignment => {
            group != "bare" || assignment_is_secret(secret, text.get(range.end..).unwrap_or(""))
        }
    }
}

/// A bare `password=` value is a secret unless it reads as code: `token = next_token(…)`,
/// `password: String`, `token == other`, `secret = config.secret`. Quoted values are always
/// redacted: a literal after `password=` is exactly what must not be stored.
fn assignment_is_secret(value: &str, after: &str) -> bool {
    const SYMBOLS: &[u8] = b"!@#$%^*+/~?=";
    if value.starts_with('=') || after.starts_with(['(', '[']) || is_dotted_identifier(value) {
        return false;
    }
    let has_digit = value.bytes().any(|byte| byte.is_ascii_digit());
    let has_symbol = value.bytes().any(|byte| SYMBOLS.contains(&byte));
    has_digit || has_symbol || value.len() >= 32
}

/// `config.secret`, `self.token`: a path through fields, not a value.
fn is_dotted_identifier(value: &str) -> bool {
    let is_identifier = |part: &str| {
        part.bytes()
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    };
    value.contains('.') && value.split('.').all(is_identifier)
}

#[cfg(test)]
mod tests {
    use super::{COMPILED, RULES, redact_json_text, redact_text};

    /// One planted secret per rule: the secret must vanish and the rule's marker must appear.
    /// Every rule must have a case here.
    const PLANTED: &[(&str, &str, &str)] = &[
        (
            "private-key",
            "key:\n-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEAplanted\n-----END RSA PRIVATE KEY-----\ndone",
            "MIIEowIBAAKCAQEAplanted",
        ),
        (
            "anthropic-key",
            "export ANTHROPIC=sk-ant-api03-PLANTEDkey0123456789abcdef",
            "sk-ant-api03-PLANTEDkey0123456789abcdef",
        ),
        (
            "openrouter-key",
            "use sk-or-v1-0123456789abcdefPLANTED0123456789 here",
            "sk-or-v1-0123456789abcdefPLANTED0123456789",
        ),
        (
            "openai-key",
            "OPENAI sk-proj-Ab3dEf6hIj9kLmNoPq2sTuVwXyZ planted",
            "sk-proj-Ab3dEf6hIj9kLmNoPq2sTuVwXyZ",
        ),
        (
            "stripe-key",
            "stripe sk_live_51PLANTEDabcdefghij",
            "sk_live_51PLANTEDabcdefghij",
        ),
        (
            "elevenlabs-key",
            "xi key sk_3f9a0b7c1d2e4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c0d1e",
            "sk_3f9a0b7c1d2e4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c0d1e",
        ),
        (
            "github-token",
            "git push with ghp_PLANTED0123456789abcdefghijklmnopqrstu",
            "ghp_PLANTED0123456789abcdefghijklmnopqrstu",
        ),
        (
            "aws-access-key-id",
            concat!("aws id AKIA", "IOSFODNN7PLANTED ok"),
            concat!("AKIA", "IOSFODNN7PLANTED"),
        ),
        (
            "aws-secret-key",
            concat!(
                "aws_secret_access_key = wJal",
                "rXUtnFEMI/K7MDENG/bPxRfiCYPLANTEDKEY"
            ),
            concat!("wJal", "rXUtnFEMI/K7MDENG/bPxRfiCYPLANTEDKEY"),
        ),
        (
            "google-api-key",
            "maps AIzaSyPLANTED0123456789abcdefghijklmnop key",
            "AIzaSyPLANTED0123456789abcdefghijklmnop",
        ),
        (
            "slack-token",
            concat!("slack xoxb", "-123456789012-PLANTEDtoken"),
            concat!("xoxb", "-123456789012-PLANTEDtoken"),
        ),
        (
            "jwt",
            "cookie eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJwbGFudGVkIn0.c2lnbmF0dXJlcGxhbnRlZA end",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJwbGFudGVkIn0.c2lnbmF0dXJlcGxhbnRlZA",
        ),
        (
            "url-password",
            "DATABASE_URL=postgres://app:Planted-Pass99@db.internal:5432/app",
            "Planted-Pass99",
        ),
        (
            "authorization",
            "Authorization: Basic dXNlcjpQbGFudGVkUGFzcw==",
            "dXNlcjpQbGFudGVkUGFzcw==",
        ),
        (
            "bearer-token",
            "curl -H 'X: Bearer plantedOpaque0123456789token'",
            "plantedOpaque0123456789token",
        ),
        (
            "api-key-header",
            "xi-api-key: Planted0123456789abcdef",
            "Planted0123456789abcdef",
        ),
        (
            "assigned-secret",
            "db: password=hunter22 user=admin",
            "hunter22",
        ),
    ];

    #[test]
    fn every_rule_compiles() {
        assert_eq!(COMPILED.rules.len(), RULES.len());
        assert_eq!(COMPILED.set.len(), RULES.len());
    }

    #[test]
    fn a_planted_secret_per_rule_is_redacted() {
        for rule in RULES {
            assert!(
                PLANTED.iter().any(|(name, ..)| *name == rule.name),
                "rule {} has no planted-secret case",
                rule.name
            );
        }
        for (name, text, secret) in PLANTED {
            let redacted = redact_text(text);
            assert!(
                !redacted.as_str().contains(secret),
                "{name}: {}",
                redacted.as_str()
            );
            assert!(
                redacted.as_str().contains(&format!("[redacted:{name}]")),
                "{name}: {}",
                redacted.as_str()
            );
            assert!(redacted.count() >= 1);
        }
    }

    #[test]
    fn assignments_keep_the_name_and_redact_quoted_and_env_values() {
        for (text, secret) in [
            (r#"{"password": "correct horse battery"}"#, "correct horse"),
            ("client_secret: 'Zm9vYmFy'", "Zm9vYmFy"),
            ("DB_PASSWORD=s3cr3t!", "s3cr3t!"),
            ("api_key = 1234567890abcdef", "1234567890abcdef"),
            ("the reset token: 482910 expires soon", "482910"),
        ] {
            let redacted = redact_text(text);
            assert!(!redacted.as_str().contains(secret), "{text}");
            assert!(redacted.as_str().contains("[redacted:"), "{text}");
        }
        assert_eq!(
            redact_text("DB_PASSWORD=s3cr3t!").as_str(),
            "DB_PASSWORD=[redacted:assigned-secret]"
        );
    }

    #[test]
    fn ordinary_code_and_prose_are_left_alone() {
        let untouched = [
            "let token = lexer.next_token();",
            "fn password_hash(password: &str) -> Hash {",
            "if token == Token::Eof { return; }",
            "pub secret: Option<Secret>,",
            "let secret = Secret::new(value);",
            "self.token = self.lexer.peek;",
            "const token: string = getToken();",
            "password: String,",
            "The token bucket refills at a steady rate.",
            "Rotate the secret: it should change every 90 days.",
            "HTTP Basic authentication sends a password: required for the admin page.",
            "Use bearer authentication between services.",
            "sk-learn-compat is a scikit-learn shim; risk-assessment-framework too.",
            "The tokenizer splits on whitespace; passwords are hashed with argon2.",
            "Redis keeps keys in memory; AKIA is the prefix of AWS key ids.",
            "Open http://localhost:11434/v1 for Ollama.",
            "We use Kafka with 12 partitions and 3 replicas.",
            "getUserByID returns the user; service.go L42 handles it.",
        ];
        for text in untouched {
            let redacted = redact_text(text);
            assert_eq!(redacted.as_str(), text);
            assert_eq!(redacted.count(), 0, "{text}");
        }
    }

    #[test]
    fn json_strings_are_redacted_in_place_and_the_shape_is_kept() {
        let json = r#"{"v":1,"t":5,"k":"ui","event":{"type":"Caption","who":"User","text":"my key is sk-ant-api03-PLANTEDkey0123456789abcdef","is_final":true}}"#;
        let redacted = redact_json_text(json);
        assert_eq!(
            redacted.as_str(),
            r#"{"v":1,"t":5,"k":"ui","event":{"type":"Caption","who":"User","text":"my key is [redacted:anthropic-key]","is_final":true}}"#
        );
        let pem = serde_json::json!({
            "lines": ["-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgPLANTED\n-----END PRIVATE KEY-----"]
        })
        .to_string();
        let redacted = redact_json_text(&pem);
        assert!(!redacted.as_str().contains("PLANTED"));
        let back: serde_json::Value =
            serde_json::from_str(redacted.as_str()).unwrap_or(serde_json::Value::Null);
        assert_eq!(back["lines"][0], "[redacted:private-key]");
        let clean = r#"{"a":"plain \"quoted\" text","b":[1,2],"c":"é ✓"}"#;
        let redacted = redact_json_text(clean);
        assert_eq!((redacted.as_str(), redacted.count()), (clean, 0));
    }

    #[test]
    fn a_key_file_split_into_lines_is_redacted_line_by_line() {
        let card = serde_json::json!({
            "lines": [
                "# deploy key",
                "-----BEGIN OPENSSH PRIVATE KEY-----",
                "b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQPLANTED",
                "AAAAC3NzaC1lZDI1NTE5AAAAIPLANTEDsecondline",
                "-----END OPENSSH PRIVATE KEY-----",
                "after the key"
            ]
        })
        .to_string();
        let redacted = redact_json_text(&card);
        assert!(
            !redacted.as_str().contains("PLANTED"),
            "{}",
            redacted.as_str()
        );
        let back: serde_json::Value =
            serde_json::from_str(redacted.as_str()).unwrap_or(serde_json::Value::Null);
        assert_eq!(back["lines"][0], "# deploy key");
        assert_eq!(back["lines"][3], "[redacted:private-key]");
        assert_eq!(back["lines"][5], "after the key");
    }

    #[test]
    fn json_redaction_survives_malformed_input() {
        for text in [
            r#"{"text":"sk-ant-api03-PLANTEDkey0123456789abcdef"#,
            r#"{"text":"bad \x escape sk-ant-api03-PLANTEDkey0123456789abcdef"}"#,
            "not json at all sk-ant-api03-PLANTEDkey0123456789abcdef",
            "\"\\",
            "\"\\é\"",
        ] {
            let redacted = redact_json_text(text);
            assert!(!redacted.as_str().contains("PLANTEDkey"), "{text}");
        }
    }
}
