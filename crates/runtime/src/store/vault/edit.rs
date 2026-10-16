//! Edits to files you own (D-042, LLD §3.6). The memory files, a project's task list and a
//! digest are yours to edit by hand, so the store changes them one line at a time: it appends a
//! line, ticks a box, or removes a line by its id, and leaves every other byte as you wrote it.
//!
//! What the store adds is redacted (I8): an [`ItemLine`] is built from [`Redacted`] text. What
//! it keeps is never run through redaction, because rewriting your file through it would
//! replace your own words for good: a secret you pasted stays in your file, and the index
//! redacts it on the way in.

use std::io;
use std::path::Path;

use super::files::{FileRead, read_text};
use super::markdown::{append_line, remove_item, render_item, set_checked};
use crate::store::ids::ItemId;
use crate::store::redact::Redacted;

/// A file's text exactly as it is on disk: whole, and valid UTF-8. Only [`read_editable`]
/// makes one, so an edit always starts from the bytes you wrote.
#[derive(Debug)]
pub(crate) struct OnDisk(String);

/// What reading a file the store may edit found.
#[derive(Debug)]
pub(crate) enum Editable {
    Missing,
    Text(OnDisk),
    /// Larger than the bound, or not UTF-8: the store leaves it as it is rather than mangle it.
    LeftAlone,
}

/// Reads a file the store may edit, of at most `max_bytes`.
pub(crate) fn read_editable(path: &Path, max_bytes: u64) -> io::Result<Editable> {
    Ok(match read_text(path, max_bytes)? {
        FileRead::Missing => Editable::Missing,
        FileRead::Text { text, lossy: false } => Editable::Text(OnDisk(text)),
        FileRead::Text { lossy: true, .. } | FileRead::TooLarge { .. } => Editable::LeftAlone,
    })
}

/// One item line the store writes: `- [ ] text [[link]] ^t27`, from redacted text and links.
/// The id is the store's own and never goes through redaction: an assignment rule could take
/// `password= ^t123456` for a value and eat the id, and then the line could never be undone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ItemLine(String);

impl ItemLine {
    /// `text` and `links` must already be one line each ([`Redacted::one_line`]).
    pub(crate) fn new(
        checked: Option<bool>,
        text: &Redacted,
        links: &[Redacted],
        id: ItemId,
    ) -> Self {
        let links: Vec<String> = links.iter().map(|link| link.as_str().to_owned()).collect();
        Self(render_item(checked, text.as_str(), &links, Some(id)))
    }
}

/// A file's new text: what was on disk with only the store's own change, or a new file made of
/// redacted text alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Edited(String);

impl Edited {
    /// A new file: its header, then the line.
    pub(crate) fn create(header: &Redacted, line: &ItemLine) -> Self {
        Self(format!("{}{}\n", header.as_str(), line.0))
    }

    /// Appends the line. Also returns whether a newline had to be added before it, so removing
    /// the line restores the file byte for byte.
    pub(crate) fn append(file: &OnDisk, line: &ItemLine) -> (Self, bool) {
        let (text, added_newline) = append_line(&file.0, &line.0);
        (Self(text), added_newline)
    }

    /// Removes the item line with `id`; `None` when no line has it.
    pub(crate) fn remove(file: &OnDisk, id: ItemId, added_newline: bool) -> Option<Self> {
        remove_item(&file.0, id, added_newline).map(|(text, _removed)| Self(text))
    }

    /// Ticks or unticks the checklist item with `id`: one character changes. `None` when no
    /// checklist line has that id.
    pub(crate) fn set_checked(file: &OnDisk, id: ItemId, checked: bool) -> Option<Self> {
        set_checked(&file.0, id, checked).map(Self)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::{Edited, ItemLine, OnDisk};
    use crate::store::ids::{ItemId, ItemKind};
    use crate::store::redact::redact_text;

    /// D-042: the store's line is redacted, your own bytes are not touched.
    #[test]
    fn an_edit_redacts_only_what_the_store_adds() {
        let yours = OnDisk("# Facts\n\n* staging password=Hunter2Hunter2 ^m40\n".to_owned());
        let id = ItemId::new(ItemKind::Memory, 41);
        let text = redact_text("prod uses token=ghp_PLANTED0123456789abcdefghijklmnopqrstu");
        let line = ItemLine::new(None, &text.one_line(200), &[], id);
        let (appended, added_newline) = Edited::append(&yours, &line);
        assert!(!added_newline);
        assert!(
            appended
                .as_str()
                .starts_with("# Facts\n\n* staging password=Hunter2Hunter2 ^m40\n"),
            "{}",
            appended.as_str()
        );
        assert!(!appended.as_str().contains("ghp_PLANTED"));
        assert!(
            appended.as_str().ends_with(" ^m41\n"),
            "{}",
            appended.as_str()
        );
        let back = OnDisk(appended.as_str().to_owned());
        assert_eq!(
            Edited::remove(&back, id, added_newline).map(|edited| edited.0),
            Some(yours.0)
        );
    }

    /// An id at the end of a line that reads like an assignment survives: it is never redacted.
    #[test]
    fn the_store_id_is_never_redacted_away() {
        let id = ItemId::new(ItemKind::Memory, 123_456);
        let line = ItemLine::new(None, &redact_text("the password="), &[], id);
        assert!(line.0.ends_with(" ^m123456"), "{}", line.0);
        assert!(
            !redact_text(&line.0).as_str().ends_with(" ^m123456"),
            "redacting the whole line would take the id for a password"
        );
    }
}
