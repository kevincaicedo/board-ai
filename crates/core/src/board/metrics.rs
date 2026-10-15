//! Label widths for the handwritten font (LLD §2.5). The core outputs final geometry, so it needs
//! to know how wide a label will be when the UI writes it.
//!
//! The advances approximate Excalifont, in thousandths of an em, for printable ASCII. They are
//! deliberately on the wide side: a box a little too wide looks hand-drawn, a label spilling out
//! of its box looks broken. Replacing them with advances read from the bundled font file is a
//! follow-up; only this table changes.

/// Advance widths for `' '` (0x20) through `'~'` (0x7E), in thousandths of an em.
const ASCII_ADVANCES: [u16; 95] = [
    // space ! " # $ % & ' ( ) * + , - . /
    330, 280, 420, 720, 620, 780, 720, 230, 360, 360, 500, 620, 280, 480, 260, 500,
    // 0 1 2 3 4 5 6 7 8 9
    620, 480, 610, 590, 630, 600, 610, 580, 610, 610, // : ; < = > ? @
    280, 290, 580, 620, 580, 560, 880, // A B C D E F G H I J K L M
    700, 660, 650, 690, 620, 600, 690, 720, 340, 520, 670, 580, 840,
    // N O P Q R S T U V W X Y Z
    730, 720, 630, 730, 660, 620, 640, 710, 690, 940, 680, 650, 640, // [ \ ] ^ _ `
    380, 500, 380, 560, 600, 340, // a b c d e f g h i j k l m
    590, 600, 540, 600, 570, 420, 600, 600, 290, 320, 560, 290, 860,
    // n o p q r s t u v w x y z
    600, 590, 600, 600, 440, 520, 440, 600, 560, 820, 570, 570, 540, // { | } ~
    400, 280, 400, 620,
];

/// Advance for characters outside the table (accents, symbols).
const DEFAULT_ADVANCE: u16 = 640;

/// Advance for wide scripts (CJK and similar), which take a full em.
const WIDE_ADVANCE: u16 = 1_000;

/// Where wide scripts begin (Hangul Jamo); everything above it is treated as full width.
const WIDE_SCRIPTS_BEGIN: char = '\u{1100}';

/// Font size of node and edge labels, in world units.
pub const LABEL_FONT_SIZE: u32 = 20;

/// Line height of a label, in world units.
pub const LABEL_LINE_HEIGHT: u32 = 25;

fn advance(ch: char) -> u16 {
    let code = u32::from(ch);
    if (0x20..=0x7E).contains(&code) {
        let index = usize::try_from(code - 0x20).unwrap_or(0);
        return ASCII_ADVANCES
            .get(index)
            .copied()
            .unwrap_or(DEFAULT_ADVANCE);
    }
    if ch >= WIDE_SCRIPTS_BEGIN && !matches!(ch, '…' | '–' | '—' | '’' | '‘' | '“' | '”')
    {
        return WIDE_ADVANCE;
    }
    DEFAULT_ADVANCE
}

/// Width of `text` written at `font_size`, rounded up to a whole unit.
#[must_use]
pub fn text_width(text: &str, font_size: u32) -> u32 {
    let thousandths: u64 = text.chars().map(|ch| u64::from(advance(ch))).sum();
    let width = (thousandths * u64::from(font_size)).div_ceil(1_000);
    u32::try_from(width).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::{LABEL_FONT_SIZE, text_width};

    #[test]
    fn wider_letters_measure_wider() {
        assert_eq!(text_width("", LABEL_FONT_SIZE), 0);
        assert!(text_width("mmmm", LABEL_FONT_SIZE) > text_width("iiii", LABEL_FONT_SIZE));
        assert!(text_width("Load balancer", LABEL_FONT_SIZE) > 100);
        assert!(text_width("Load balancer", LABEL_FONT_SIZE) < 200);
        assert_eq!(text_width("缓存", 20), 40);
        assert_eq!(text_width("A", 40), 2 * text_width("A", 20));
    }
}
