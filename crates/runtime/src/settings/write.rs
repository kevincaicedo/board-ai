//! Writes the file's fields back as TOML (D-102). Only fields the file sets are written, so a
//! default stays a default and moves with the code. A short writer instead of a TOML
//! serializer: the shape is small and fixed, and every string goes through one escaper.

use std::fmt::Write as _;

use super::raw::{RawAudio, RawHarness, RawLlm, RawProvider, RawSettings, RawVoice};

/// The file's text: top-level keys, then one table per section that sets anything.
pub(super) fn to_toml(raw: &RawSettings) -> String {
    let mut out = String::new();
    if let Some(done) = raw.setup_complete {
        let _ = writeln!(out, "setup_complete = {done}");
    }
    pair(&mut out, "vault", raw.vault.as_deref());
    if let Some(roots) = &raw.code_roots {
        let items: Vec<String> = roots.iter().map(|root| quoted(root)).collect();
        let _ = writeln!(out, "code_roots = [{}]", items.join(", "));
    }
    pair(&mut out, "mic", raw.mic.as_deref());
    if let Some(RawVoice { id }) = &raw.voice {
        table(&mut out, "voice", &[("id", id.as_deref())]);
    }
    if let Some(RawAudio { input, output }) = &raw.audio {
        table(
            &mut out,
            "audio",
            &[("input", input.as_deref()), ("output", output.as_deref())],
        );
    }
    if let Some(talker) = &raw.talker {
        llm(&mut out, "talker", talker);
    }
    if let Some(scribe) = &raw.scribe {
        llm(&mut out, "scribe", scribe);
    }
    if let Some(stt) = &raw.stt {
        provider(&mut out, "stt", stt);
    }
    if let Some(tts) = &raw.tts {
        provider(&mut out, "tts", tts);
    }
    if let Some(RawHarness {
        kind,
        model,
        research_model,
    }) = &raw.harness
    {
        let fields = [
            ("kind", kind.as_deref()),
            ("model", model.as_deref()),
            ("research_model", research_model.as_deref()),
        ];
        table(&mut out, "harness", &fields);
    }
    out
}

fn llm(out: &mut String, name: &str, raw: &RawLlm) {
    let fields = [
        ("kind", raw.kind.as_deref()),
        ("model", raw.model.as_deref()),
        ("effort", raw.effort.as_deref()),
        ("base_url", raw.base_url.as_deref()),
        ("secret", raw.secret.as_deref()),
    ];
    let mut set = table(out, name, &fields);
    if let Some(tokens) = raw.max_tokens {
        if !set {
            let _ = writeln!(out, "\n[{name}]");
            set = true;
        }
        let _ = writeln!(out, "max_tokens = {tokens}");
    }
    if let Some(route) = &raw.route {
        if !set {
            let _ = writeln!(out, "\n[{name}]");
        }
        let items: Vec<String> = route.iter().map(|slug| quoted(slug)).collect();
        let _ = writeln!(out, "route = [{}]", items.join(", "));
    }
}

fn provider(out: &mut String, name: &str, raw: &RawProvider) {
    let fields = [
        ("kind", raw.kind.as_deref()),
        ("model", raw.model.as_deref()),
        ("base_url", raw.base_url.as_deref()),
        ("secret", raw.secret.as_deref()),
    ];
    table(out, name, &fields);
}

/// Writes `[name]` and its set fields; nothing when none is set. Returns whether it wrote.
fn table(out: &mut String, name: &str, fields: &[(&str, Option<&str>)]) -> bool {
    if fields.iter().all(|(_, value)| value.is_none()) {
        return false;
    }
    let _ = writeln!(out, "\n[{name}]");
    for (key, value) in fields {
        pair(out, key, *value);
    }
    true
}

fn pair(out: &mut String, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        let _ = writeln!(out, "{key} = {}", quoted(value));
    }
}

/// A TOML basic string: quotes, backslashes and control characters escaped.
fn quoted(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            control if control.is_control() => {
                let _ = write!(out, "\\u{:04X}", u32::from(control));
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::{quoted, to_toml};
    use crate::settings::raw::read_raw;

    #[test]
    fn strings_survive_quotes_backslashes_and_control_characters() {
        let awkward = "a \"quoted\" C:\\path\twith\nlines, \u{7f} and ünïcode";
        let text = format!("vault = {}\n", quoted(awkward));
        let raw = read_raw(&text).expect("the escaped string parses");
        assert_eq!(raw.vault.as_deref(), Some(awkward));
    }

    #[test]
    fn a_written_file_reads_back_field_for_field() {
        let text = r#"
setup_complete = true
vault = "~/Notes/BoardAI"
code_roots = ["~/code", "/opt/src"]
mic = "push_to_talk"

[voice]
id = "af_heart"

[audio]
input = "built_in"

[talker]
kind = "openai_compatible"
model = "qwen3.5:2b"
effort = "none"
max_tokens = 4000

[scribe]
kind = "openai_compatible"
model = "qwen/qwen3.8-27b"
base_url = "https://openrouter.ai/api/v1"
secret = "openrouter"
route = ["cerebras", "deepinfra/turbo"]
max_tokens = 2000

[stt]
model = "small.en"

[tts]
kind = "elevenlabs"
secret = "elevenlabs"

[harness]
kind = "pi"
research_model = "opus"
"#;
        let raw = read_raw(text).expect("parses");
        let written = to_toml(&raw);
        let again = read_raw(&written).expect("the written file parses");
        assert_eq!(to_toml(&again), written);
        let home = std::path::Path::new("/Users/someone");
        assert_eq!(
            crate::settings::Settings::parse(&written, home),
            crate::settings::Settings::parse(text, home)
        );
    }

    #[test]
    fn an_empty_file_writes_nothing() {
        let raw = read_raw("").expect("parses");
        assert_eq!(to_toml(&raw), "");
    }
}
