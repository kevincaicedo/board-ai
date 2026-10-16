//! `path:line` citations in the talker's speech (PRODUCT §7.3 source chips). The prompt asks for
//! every claim about the repo to cite `file:line`; a spoken word that reads as one becomes a
//! `Cite` op anchored on it, and the engine shows a source chip when that line was read this
//! session (law 6). This only recognises the form; whether it names something read is the
//! engine's question.

/// Marks around a word that are not part of the citation ("(see `service.go:43`).").
const LEADING: &[char] = &['(', '[', '"', '\'', '`', '“', '‘', '*', '_'];
const TRAILING: &[char] = &[
    '.', ',', ';', ':', '!', '?', ')', ']', '"', '\'', '`', '”', '’', '…', '*', '_',
];

/// The longest line number a citation carries.
const LINE_DIGITS_MAX: usize = 7;

/// Characters a cited path may hold.
fn path_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | '@' | '+')
}

/// `internal/user/service.go:43` (also `:43-58`, `:L43`) → the path and the first line. A path
/// needs a file extension or a folder, so `localhost:8080` and `12:30` are not citations.
#[must_use]
pub fn parse(word: &str) -> Option<(String, u32)> {
    let word = word.trim_start_matches(LEADING).trim_end_matches(TRAILING);
    let (path, lines) = word.rsplit_once(':')?;
    let path = path.strip_prefix("./").unwrap_or(path);
    let lines = lines.strip_prefix('L').unwrap_or(lines);
    let first = lines
        .split_once(['-', '–'])
        .map_or(lines, |(first, _)| first);
    if first.is_empty()
        || first.len() > LINE_DIGITS_MAX
        || !first.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let line: u32 = first.parse().ok().filter(|line| *line >= 1)?;
    let file = path.rsplit('/').next().unwrap_or(path);
    let extension = file.rsplit_once('.').is_some_and(|(stem, extension)| {
        !stem.is_empty() && extension.chars().any(|ch| ch.is_ascii_alphabetic())
    });
    let valid = !path.is_empty()
        && !path.contains("//")
        && path.chars().all(path_char)
        && (extension || path.contains('/'));
    valid.then(|| (path.to_owned(), line))
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn reads_file_line_citations_and_nothing_else() {
        let cited = |word: &str| parse(word).map(|(path, line)| format!("{path}@{line}"));
        for (word, expected) in [
            (
                "internal/user/service.go:43",
                Some("internal/user/service.go@43"),
            ),
            ("service.go:43.", Some("service.go@43")),
            ("(`./src/lib.rs:7-12`),", Some("src/lib.rs@7")),
            ("handler.go:L88", Some("handler.go@88")),
            ("crates/core/Makefile:3", Some("crates/core/Makefile@3")),
            ("localhost:8080", None),
            ("12:30", None),
            ("http://example.com:80", None),
            ("service.go:", None),
            ("service.go:0", None),
            ("service.go:abc", None),
            ("Note:", None),
            ("a.b:12345678", None),
        ] {
            assert_eq!(cited(word).as_deref(), expected, "{word}");
        }
    }
}
