//! Letter-to-sound rules: the last resort for a word that neither the lexicon nor its
//! derivations (plurals, compounds, acronyms) cover. espeak-ng would do this better, but it is
//! GPL; these are a small table in the spirit of the public-domain NRL rules (Elovitz et al.,
//! 1976), written for misaki's American phoneme set. Expect intelligible, not perfect, output.
//!
//! The word is read left to right; at each letter the first rule whose letters match and whose
//! contexts hold is applied. Primary stress goes before the first vowel, English's most common
//! pattern.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Context {
    Any,
    Start,
    End,
    Vowel,
    Consonant,
    /// `e`, `i` or `y`: softens `c` and `g`.
    FrontVowel,
    /// One consonant, then a final `e` (optionally `es`/`ed`): the "magic e" that makes the
    /// vowel long.
    MagicE,
}

struct Rule {
    letters: &'static str,
    before: Context,
    after: Context,
    phonemes: &'static str,
}

const fn rule(
    letters: &'static str,
    before: Context,
    after: Context,
    phonemes: &'static str,
) -> Rule {
    Rule {
        letters,
        before,
        after,
        phonemes,
    }
}

use Context::{Any, Consonant, End, FrontVowel, MagicE, Start, Vowel};

/// Longer and more specific rules first; within a letter the first match wins.
const RULES: &[Rule] = &[
    rule("augh", Any, Any, "ɔ"),
    rule("au", Any, Any, "ɔ"),
    rule("aw", Any, Any, "ɔ"),
    rule("ai", Any, Any, "A"),
    rule("ay", Any, Any, "A"),
    rule("all", Any, End, "ɔl"),
    rule("ar", Any, Any, "ɑɹ"),
    rule("a", Any, MagicE, "A"),
    rule("a", Consonant, End, "ə"),
    rule("a", Any, Any, "æ"),
    rule("eau", Any, Any, "O"),
    rule("ee", Any, Any, "i"),
    rule("ea", Any, Any, "i"),
    rule("ei", Any, Any, "A"),
    rule("ey", Any, End, "i"),
    rule("ey", Any, Any, "A"),
    rule("ew", Any, Any, "ju"),
    rule("er", Any, Any, "əɹ"),
    rule("e", Consonant, End, ""),
    rule("e", Any, MagicE, "i"),
    rule("e", Any, Any, "ɛ"),
    rule("igh", Any, Any, "I"),
    rule("ie", Any, End, "I"),
    rule("ie", Any, Any, "i"),
    rule("ir", Any, Any, "ɜɹ"),
    rule("i", Any, MagicE, "I"),
    rule("i", Consonant, End, "i"),
    rule("i", Any, Any, "ɪ"),
    rule("ough", Any, Any, "ɔ"),
    rule("oo", Any, Any, "u"),
    rule("ou", Any, Any, "W"),
    rule("ow", Any, End, "O"),
    rule("ow", Any, Any, "W"),
    rule("oi", Any, Any, "Y"),
    rule("oy", Any, Any, "Y"),
    rule("oa", Any, Any, "O"),
    rule("or", Any, Any, "ɔɹ"),
    rule("o", Any, MagicE, "O"),
    rule("o", Any, End, "O"),
    rule("o", Any, Any, "ɑ"),
    rule("ui", Any, Any, "u"),
    rule("ur", Any, Any, "ɜɹ"),
    rule("u", Any, MagicE, "u"),
    rule("u", Any, End, "u"),
    rule("u", Any, Any, "ʌ"),
    rule("y", Start, Any, "j"),
    rule("y", Any, Vowel, "j"),
    rule("y", Consonant, End, "i"),
    rule("y", Any, Any, "ɪ"),
    rule("tch", Any, Any, "ʧ"),
    rule("tion", Any, Any, "ʃən"),
    rule("sion", Any, Any, "ʒən"),
    rule("ture", Any, Any, "ʧəɹ"),
    rule("ch", Any, Any, "ʧ"),
    rule("sh", Any, Any, "ʃ"),
    rule("th", Any, Any, "θ"),
    rule("ph", Any, Any, "f"),
    rule("wh", Any, Any, "w"),
    rule("wr", Start, Any, "ɹ"),
    rule("kn", Start, Any, "n"),
    rule("gn", Start, Any, "n"),
    rule("ck", Any, Any, "k"),
    rule("ng", Start, Any, "nɡ"),
    rule("ng", Any, Any, "ŋ"),
    rule("nk", Any, Any, "ŋk"),
    rule("qu", Any, Any, "kw"),
    rule("dge", Any, Any, "ʤ"),
    rule("gh", Start, Any, "ɡ"),
    rule("gh", Any, Any, ""),
    rule("x", Start, Any, "z"),
    rule("x", Any, Any, "ks"),
    rule("c", Any, FrontVowel, "s"),
    rule("c", Any, Any, "k"),
    rule("g", Any, FrontVowel, "ʤ"),
    rule("g", Any, Any, "ɡ"),
    rule("s", Any, Any, "s"),
    rule("j", Any, Any, "ʤ"),
    rule("r", Any, Any, "ɹ"),
    rule("q", Any, Any, "k"),
    rule("w", Any, Any, "w"),
    rule("h", Any, Any, "h"),
    rule("b", Any, Any, "b"),
    rule("d", Any, Any, "d"),
    rule("f", Any, Any, "f"),
    rule("k", Any, Any, "k"),
    rule("l", Any, Any, "l"),
    rule("m", Any, Any, "m"),
    rule("n", Any, Any, "n"),
    rule("p", Any, Any, "p"),
    rule("t", Any, Any, "t"),
    rule("v", Any, Any, "v"),
    rule("z", Any, Any, "z"),
];

/// Phoneme symbols that are vowels in misaki's set (where stress marks go).
const VOWEL_PHONEMES: &str = "AIOWYæɑɔəɛɜɪʊʌiuᵊᵻɐ";

fn is_vowel_letter(letter: u8) -> bool {
    matches!(letter, b'a' | b'e' | b'i' | b'o' | b'u' | b'y')
}

fn holds(context: Context, letters: &[u8], at: usize, before: bool) -> bool {
    let neighbor = if before {
        at.checked_sub(1).and_then(|index| letters.get(index))
    } else {
        letters.get(at)
    };
    match context {
        Any => true,
        Start => before && at == 0,
        End => !before && at >= letters.len(),
        Vowel => neighbor.is_some_and(|letter| is_vowel_letter(*letter)),
        Consonant => neighbor.is_some_and(|letter| !is_vowel_letter(*letter)),
        FrontVowel => neighbor.is_some_and(|letter| matches!(letter, b'e' | b'i' | b'y')),
        MagicE => {
            let rest = &letters[at.min(letters.len())..];
            matches!(rest, [consonant, b'e'] | [consonant, b'e', b's' | b'd'] if !is_vowel_letter(*consonant))
        }
    }
}

/// Folds common accented letters to ASCII and drops everything that is not a letter.
fn letters(word: &str) -> Vec<u8> {
    word.chars()
        .filter_map(|character| {
            let folded = match character.to_ascii_lowercase() {
                'à' | 'á' | 'â' | 'ä' | 'ã' | 'å' => 'a',
                'è' | 'é' | 'ê' | 'ë' => 'e',
                'ì' | 'í' | 'î' | 'ï' => 'i',
                'ò' | 'ó' | 'ô' | 'ö' | 'õ' => 'o',
                'ù' | 'ú' | 'û' | 'ü' => 'u',
                'ç' => 'c',
                'ñ' => 'n',
                other => other,
            };
            folded.is_ascii_lowercase().then_some(folded as u8)
        })
        .collect()
}

/// The word's phonemes by rule, with primary stress before the first vowel.
#[must_use]
pub(crate) fn pronounce(word: &str) -> String {
    let letters = letters(word);
    let mut out = String::new();
    let mut at = 0;
    while at < letters.len() {
        // A doubled consonant sounds once.
        if at > 0 && letters[at] == letters[at - 1] && !is_vowel_letter(letters[at]) {
            at += 1;
            continue;
        }
        let matched = RULES.iter().find(|rule| {
            let pattern = rule.letters.as_bytes();
            letters[at..].starts_with(pattern)
                && holds(rule.before, &letters, at, true)
                && holds(rule.after, &letters, at + pattern.len(), false)
        });
        match matched {
            Some(rule) => {
                out.push_str(rule.phonemes);
                at += rule.letters.len();
            }
            None => at += 1,
        }
    }
    if let Some((index, _)) = out
        .char_indices()
        .find(|(_, c)| VOWEL_PHONEMES.contains(*c))
    {
        out.insert(index, 'ˈ');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::pronounce;
    use crate::speech_out::kokoro::vocab::token;

    #[test]
    fn common_spellings_sound_right() {
        let cases = [
            ("cat", "kˈæt"),
            ("cake", "kˈAk"),
            ("phone", "fˈOn"),
            ("night", "nˈIt"),
            ("happy", "hˈæpi"),
            ("city", "sˈɪti"),
            ("redis", "ɹˈɛdɪs"),
            // Rules cannot know "engine x"; this is what a word with no better source gets.
            ("nginx", "nɡˈɪnks"),
            ("Postgres", "pˈɑstɡɹɛs"),
        ];
        for (word, expected) in cases {
            assert_eq!(pronounce(word), expected, "{word}");
        }
    }

    #[test]
    fn every_rule_output_is_in_the_model_vocabulary() {
        for word in [
            "kubernetes",
            "tauri",
            "postgres",
            "xylophone",
            "quickly",
            "judge",
            "ought",
        ] {
            let phonemes = pronounce(word);
            assert!(!phonemes.is_empty(), "{word}");
            assert!(
                phonemes.chars().all(|c| token(c).is_some()),
                "{word}: {phonemes}"
            );
        }
        assert_eq!(pronounce("123 !"), "");
    }
}
