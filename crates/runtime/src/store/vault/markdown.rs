//! The vault's Markdown formats, written by the store and editable by hand.
//!
//! - Front matter: `---`, then `key: value` lines, then `---`. Text values are JSON strings
//!   and lists are JSON arrays of strings, both valid YAML, so other tools read them too.
//! - Items: one list line each, with an optional checkbox and the item id last:
//!   `- [ ] Check Redis TTLs [[session:2026-10-15-143210]] ^t27`. Links are `[[…]]` groups
//!   right before the id. A hand-written line without an id is still an item.
//!
//! Parsing is line by line with no recursion, and lenient: a malformed line is body text, never
//! an error, because a person may have edited the file.

use crate::limits::VAULT_FRONT_MATTER_LINES_MAX;
use crate::store::ids::ItemId;

const FENCE: &str = "---";

/// A front matter value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FrontValue {
    Text(String),
    List(Vec<String>),
}

/// Front matter fields, in file order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct FrontMatter {
    fields: Vec<(String, FrontValue)>,
}

impl FrontMatter {
    pub(crate) fn text(&self, key: &str) -> Option<&str> {
        self.fields.iter().find_map(|(name, value)| match value {
            FrontValue::Text(text) if name == key => Some(text.as_str()),
            FrontValue::Text(_) | FrontValue::List(_) => None,
        })
    }

    pub(crate) fn list(&self, key: &str) -> Vec<String> {
        self.fields
            .iter()
            .find_map(|(name, value)| match value {
                FrontValue::List(items) if name == key => Some(items.clone()),
                FrontValue::Text(_) | FrontValue::List(_) => None,
            })
            .unwrap_or_default()
    }
}

/// Splits a file into its front matter and its body. A file without a closed block within
/// [`VAULT_FRONT_MATTER_LINES_MAX`] lines is all body.
pub(crate) fn split_front_matter(text: &str) -> (Option<FrontMatter>, &str) {
    let Some(rest) = text.strip_prefix(FENCE).and_then(|rest| {
        rest.strip_prefix('\n')
            .or_else(|| rest.strip_prefix("\r\n"))
    }) else {
        return (None, text);
    };
    let mut front = FrontMatter::default();
    let mut offset = 0;
    for _ in 0..VAULT_FRONT_MATTER_LINES_MAX {
        let line_end = rest[offset..].find('\n').map(|at| offset + at);
        let line = &rest[offset..line_end.unwrap_or(rest.len())];
        let next = line_end.map_or(rest.len(), |at| at + 1);
        if line.trim_end() == FENCE {
            return (Some(front), &rest[next..]);
        }
        if let Some((key, value)) = line.split_once(':') {
            front
                .fields
                .push((key.trim().to_owned(), parse_value(value.trim())));
        }
        if line_end.is_none() {
            break;
        }
        offset = next;
    }
    (None, text)
}

fn parse_value(raw: &str) -> FrontValue {
    if raw.starts_with('[')
        && let Ok(items) = serde_json::from_str::<Vec<String>>(raw)
    {
        return FrontValue::List(items);
    }
    if raw.starts_with('"')
        && let Ok(text) = serde_json::from_str::<String>(raw)
    {
        return FrontValue::Text(text);
    }
    FrontValue::Text(raw.to_owned())
}

/// Renders a front matter block. Text values are JSON-quoted unless they are plain tokens
/// (ids, stamps, numbers), so every value stays on one line.
pub(crate) fn render_front_matter(fields: &[(&str, FrontValue)]) -> String {
    let mut out = String::from("---\n");
    for (key, value) in fields {
        out.push_str(key);
        out.push_str(": ");
        match value {
            FrontValue::Text(text) if is_plain_token(text) => out.push_str(text),
            FrontValue::Text(text) => out.push_str(&json_string(text)),
            FrontValue::List(items) => {
                out.push_str(&serde_json::to_string(items).unwrap_or_else(|_| "[]".to_owned()));
            }
        }
        out.push('\n');
    }
    out.push_str("---\n");
    out
}

fn is_plain_token(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn json_string(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_owned())
}

/// One list item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Item<'a> {
    /// `Some(done)` for a checklist line.
    pub checked: Option<bool>,
    /// The text, links included, id excluded.
    pub text: &'a str,
    pub id: Option<ItemId>,
}

/// Parses a `- …` or `* …` list line.
pub(crate) fn parse_item(line: &str) -> Option<Item<'_>> {
    let body = line
        .trim_start()
        .strip_prefix("- ")
        .or_else(|| line.trim_start().strip_prefix("* "))?;
    let (checked, body) = if let Some(rest) = body.strip_prefix("[ ] ") {
        (Some(false), rest)
    } else if let Some(rest) = body
        .strip_prefix("[x] ")
        .or_else(|| body.strip_prefix("[X] "))
    {
        (Some(true), rest)
    } else {
        (None, body)
    };
    let body = body.trim_end();
    let (text, id) = match body.rsplit_once(" ^") {
        Some((text, id)) => match ItemId::parse(id) {
            Some(id) => (text.trim_end(), Some(id)),
            None => (body, None),
        },
        None => (body, None),
    };
    (!text.is_empty()).then_some(Item { checked, text, id })
}

/// Splits trailing `[[link]]` groups off an item's text.
pub(crate) fn split_links(text: &str) -> (&str, Vec<String>) {
    let mut rest = text.trim_end();
    let mut links = Vec::new();
    while let Some(open) = rest.strip_suffix("]]").and_then(|inner| inner.rfind("[[")) {
        let link = &rest[open + 2..rest.len() - 2];
        if link.is_empty() || link.contains("[[") {
            break;
        }
        links.push(link.to_owned());
        rest = rest[..open].trim_end();
    }
    links.reverse();
    (rest, links)
}

/// Renders one item line (without its newline). Text and links must already be one line.
pub(crate) fn render_item(
    checked: Option<bool>,
    text: &str,
    links: &[String],
    id: Option<ItemId>,
) -> String {
    let mut line = String::from("- ");
    match checked {
        Some(true) => line.push_str("[x] "),
        Some(false) => line.push_str("[ ] "),
        None => {}
    }
    line.push_str(text);
    for link in links {
        line.push_str(" [[");
        line.push_str(link);
        line.push_str("]]");
    }
    if let Some(id) = id {
        line.push_str(" ^");
        line.push_str(&id.to_string());
    }
    line
}

/// Makes text safe for one item line: whitespace runs (newlines included) become one space,
/// link brackets are broken up so they can't be read as links, and the result is cut to
/// `max_bytes` at a character boundary with `…`.
pub(crate) fn one_line(text: &str, max_bytes: usize) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let safe = joined.replace("[[", "[ [").replace("]]", "] ]");
    cut_bytes(&safe, max_bytes)
}

/// Cuts text to at most `max_bytes` (plus the `…`), at a character boundary.
pub(crate) fn cut_bytes(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_owned();
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", text[..end].trim_end())
}

/// Cuts text to at most `max_chars` characters (plus the `…`), at a word when one is near.
pub(crate) fn cut_chars(text: &str, max_chars: usize) -> String {
    let Some((end, _)) = text.char_indices().nth(max_chars) else {
        return text.to_owned();
    };
    let head = &text[..end];
    let at_word = head
        .rfind(char::is_whitespace)
        .filter(|&space| space > end / 2)
        .map_or(head, |space| &head[..space]);
    format!("{}…", at_word.trim_end())
}

/// Appends a line, adding a newline first when the file doesn't end with one. Returns the new
/// content and whether that newline was added (so removing the line can restore the file
/// byte for byte).
pub(crate) fn append_line(content: &str, line: &str) -> (String, bool) {
    let mut out = String::with_capacity(content.len() + line.len() + 2);
    out.push_str(content);
    let added_newline = !content.is_empty() && !content.ends_with('\n');
    if added_newline {
        out.push('\n');
    }
    out.push_str(line);
    out.push('\n');
    (out, added_newline)
}

/// Removes the item line with `id`. Returns the new content and the removed item's text, or
/// `None` when no line has that id. `added_newline` undoes [`append_line`]'s newline when the
/// removed line was the last one.
pub(crate) fn remove_item(
    content: &str,
    id: ItemId,
    added_newline: bool,
) -> Option<(String, String)> {
    let mut out = String::with_capacity(content.len());
    let mut removed = None;
    let mut removed_last = false;
    for line in content.split_inclusive('\n') {
        let matches = removed.is_none()
            && parse_item(line.trim_end_matches(['\n', '\r']))
                .is_some_and(|item| item.id == Some(id));
        if matches {
            removed =
                parse_item(line.trim_end_matches(['\n', '\r'])).map(|item| item.text.to_owned());
            removed_last = true;
        } else {
            out.push_str(line);
            removed_last = false;
        }
    }
    let removed = removed?;
    if removed_last && added_newline && out.ends_with('\n') {
        out.pop();
    }
    Some((out, removed))
}

/// Sets the checkbox of the item with `id`. `None` when no checklist line has that id.
pub(crate) fn set_checked(content: &str, id: ItemId, checked: bool) -> Option<String> {
    let mut out = String::with_capacity(content.len());
    let mut found = false;
    for line in content.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\n', '\r']);
        let item = parse_item(bare).filter(|item| item.id == Some(id) && item.checked.is_some());
        match item {
            Some(item) if !found => {
                found = true;
                out.push_str(&render_item(Some(checked), item.text, &[], item.id));
                out.push_str(&line[bare.len()..]);
            }
            Some(_) | None => out.push_str(line),
        }
    }
    found.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::{
        FrontValue, append_line, cut_chars, one_line, parse_item, remove_item, render_front_matter,
        render_item, set_checked, split_front_matter, split_links,
    };
    use crate::store::ids::ItemId;

    #[test]
    fn front_matter_round_trips() {
        let block = render_front_matter(&[
            ("id", FrontValue::Text("n12".into())),
            ("title", FrontValue::Text("Cache: \"aside\"".into())),
            (
                "links",
                FrontValue::List(vec!["src/a.rs:4".into(), "lb".into()]),
            ),
        ]);
        let file = format!("{block}Body line\n");
        let (front, body) = split_front_matter(&file);
        let front = front.unwrap_or_default();
        assert_eq!(front.text("id"), Some("n12"));
        assert_eq!(front.text("title"), Some("Cache: \"aside\""));
        assert_eq!(front.list("links"), vec!["src/a.rs:4", "lb"]);
        assert_eq!(body, "Body line\n");
        let unclosed = "---\nid: n1\nno end";
        assert_eq!(split_front_matter(unclosed), (None, unclosed));
        assert_eq!(split_front_matter("plain"), (None, "plain"));
    }

    #[test]
    fn items_parse_with_checkboxes_links_and_ids() {
        let line = "- [ ] Check Redis TTLs [[session:2026-10-15-1]] [[src/a.rs:4]] ^t27";
        let item = parse_item(line);
        assert_eq!(item.as_ref().map(|item| item.checked), Some(Some(false)));
        assert_eq!(item.as_ref().and_then(|item| item.id), ItemId::parse("t27"));
        let (title, links) = split_links(item.map(|item| item.text).unwrap_or_default());
        assert_eq!(title, "Check Redis TTLs");
        assert_eq!(links, vec!["session:2026-10-15-1", "src/a.rs:4"]);
        assert_eq!(
            render_item(Some(false), title, &links, ItemId::parse("t27")),
            line
        );
        let hand = parse_item("* [X] done by hand");
        assert_eq!(
            hand.map(|item| (item.checked, item.text, item.id)),
            Some((Some(true), "done by hand", None))
        );
        assert_eq!(
            parse_item("- note about x^2 ^not-an-id").map(|item| item.text),
            Some("note about x^2 ^not-an-id")
        );
        assert_eq!(parse_item("plain paragraph"), None);
        assert_eq!(parse_item("- "), None);
    }

    #[test]
    fn one_line_text_cannot_fake_links_or_ids_structure() {
        assert_eq!(one_line("a\n b\t[[x]]", 100), "a b [ [x] ]");
        assert_eq!(one_line("ééé", 3), "é…");
        assert_eq!(cut_chars("one two three four", 9), "one two…");
    }

    #[test]
    fn append_and_remove_restore_the_file_byte_for_byte() {
        for original in ["# Facts\n\n- old ^m1\n", "# Facts\n\n- old ^m1", ""] {
            let (appended, added) = append_line(original, "- new fact ^m2");
            let id =
                ItemId::parse("m2").unwrap_or(ItemId::new(crate::store::ids::ItemKind::Memory, 0));
            let (restored, text) = remove_item(&appended, id, added).unwrap_or_default();
            assert_eq!(restored, original);
            assert_eq!(text, "new fact");
        }
        assert_eq!(
            remove_item(
                "- a ^m1\n",
                ItemId::new(crate::store::ids::ItemKind::Memory, 9),
                false
            ),
            None
        );
    }

    #[test]
    fn checkboxes_toggle_by_id() {
        let content = "# P\n\n- [ ] a ^p1\n- [ ] b ^p2\n";
        let checked = set_checked(
            content,
            ItemId::new(crate::store::ids::ItemKind::Proposal, 2),
            true,
        );
        assert_eq!(
            checked.as_deref(),
            Some("# P\n\n- [ ] a ^p1\n- [x] b ^p2\n")
        );
        assert_eq!(
            set_checked(
                content,
                ItemId::new(crate::store::ids::ItemKind::Proposal, 3),
                true
            ),
            None
        );
    }
}
