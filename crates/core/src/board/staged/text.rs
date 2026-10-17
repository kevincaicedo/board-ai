//! Wrapping a diagram's notes into lines that fit their container (D-187).

use crate::board::metrics::text_width;
use crate::limits::DIAGRAM_NOTE_LINES_MAX;

/// Words of `text` in lines no wider than `width` at `font_size`, at most
/// [`DIAGRAM_NOTE_LINES_MAX`]; a longer note's last line ends with "…". A word wider than the
/// line stands alone on it.
#[must_use]
pub fn wrap(text: &str, width: u32, font_size: u32) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut cut = false;
    for word in text.split_whitespace() {
        let fits = lines
            .last()
            .is_some_and(|line: &String| text_width(&format!("{line} {word}"), font_size) <= width);
        if fits {
            if let Some(line) = lines.last_mut() {
                line.push(' ');
                line.push_str(word);
            }
            continue;
        }
        if lines.len() == DIAGRAM_NOTE_LINES_MAX {
            cut = true;
            break;
        }
        lines.push(word.to_owned());
    }
    if cut && let Some(last) = lines.last_mut() {
        last.push('…');
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::wrap;
    use crate::board::metrics::text_width;
    use crate::limits::DIAGRAM_NOTE_LINES_MAX;

    #[test]
    fn notes_wrap_at_their_width_and_long_ones_are_cut() {
        let lines = wrap("size classes: powers of two, 64 KiB to 2 MiB", 180, 13);
        assert!(lines.len() >= 2, "{lines:?}");
        assert!(
            lines.iter().all(|line| text_width(line, 13) <= 180),
            "{lines:?}"
        );
        assert_eq!(
            lines.join(" "),
            "size classes: powers of two, 64 KiB to 2 MiB"
        );
        let long = "word ".repeat(200);
        let cut = wrap(&long, 100, 13);
        assert_eq!(cut.len(), DIAGRAM_NOTE_LINES_MAX);
        assert!(cut.last().is_some_and(|line| line.ends_with('…')));
        assert_eq!(wrap("antidisestablishmentarianism", 20, 13).len(), 1);
        assert!(wrap("   ", 100, 13).is_empty());
    }
}
