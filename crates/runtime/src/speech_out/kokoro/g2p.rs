//! Text to Kokoro phoneme tokens without espeak-ng (LLD §3.4): misaki's lexicon first, then
//! what misaki derives from it (possessives, plurals, past tenses, gerunds), acronyms spelled
//! out, camelCase and compound words split into known parts, numbers read as words, and the
//! letter-to-sound rules last.
//!
//! Each word keeps the byte offset where it starts in the clause and the index of its first
//! token, so the model's per-token durations become word timings.

use super::lexicon::Lexicon;
use super::numbers;
use super::rules;
use super::vocab::token;
use crate::limits::G2P_SPLIT_DEPTH_MAX;

/// Tokens for one clause, and where each word's tokens begin.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Phonemized {
    pub tokens: Vec<i64>,
    /// `(byte offset in the clause, index of the word's first token)`, in order.
    pub words: Vec<(u32, usize)>,
}

impl Phonemized {
    fn push_symbols(&mut self, symbols: &str) {
        self.tokens.extend(symbols.chars().filter_map(token));
    }

    fn space(&mut self) {
        if self
            .tokens
            .last()
            .is_some_and(|last| Some(*last) != token(' '))
        {
            self.push_symbols(" ");
        }
    }
}

/// Punctuation Kokoro has tokens for; it shapes the pauses and intonation.
const SPOKEN_PUNCTUATION: &str = ",.!?;:—…\"()“”";

/// Symbols read as words.
fn symbol_word(symbol: char) -> Option<&'static str> {
    match symbol {
        '%' => Some("percent"),
        '&' => Some("and"),
        '+' => Some("plus"),
        '@' => Some("at"),
        '=' => Some("equals"),
        _ => None,
    }
}

fn is_word_character(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '\'' | '’')
}

/// Phonemizes a clause.
#[must_use]
pub(crate) fn phonemize(text: &str, lexicon: &Lexicon) -> Phonemized {
    let mut out = Phonemized::default();
    let mut characters = text.char_indices().peekable();
    while let Some((offset, character)) = characters.next() {
        if is_word_character(character) {
            let mut end = offset + character.len_utf8();
            while let Some(&(next_offset, next)) = characters.peek() {
                // A decimal point inside a number stays in the word ("3.14").
                let decimal = next == '.'
                    && text[..next_offset].ends_with(|c: char| c.is_ascii_digit())
                    && text[next_offset + 1..].starts_with(|c: char| c.is_ascii_digit());
                if !is_word_character(next) && !decimal {
                    break;
                }
                end = next_offset + next.len_utf8();
                characters.next();
            }
            out.space();
            let begin = u32::try_from(offset).unwrap_or(u32::MAX);
            out.words.push((begin, out.tokens.len()));
            out.push_symbols(&word(&text[offset..end], lexicon));
        } else if SPOKEN_PUNCTUATION.contains(character) {
            out.push_symbols(&character.to_string());
        } else if let Some(spoken) = symbol_word(character) {
            out.space();
            out.push_symbols(lexicon.get(spoken).unwrap_or_default());
        } else {
            out.space();
        }
    }
    while out.tokens.last() == token(' ').as_ref() {
        out.tokens.pop();
    }
    out
}

/// One written word (letters, digits, apostrophes) as phonemes, words separated by spaces.
pub(crate) fn word(written: &str, lexicon: &Lexicon) -> String {
    let normalized = written.replace('’', "'");
    let parts = split_letters_and_digits(&normalized);
    let spoken: Vec<String> = parts
        .iter()
        .map(|part| {
            if part.starts_with(|c: char| c.is_ascii_digit()) {
                let words = numbers::words(part).unwrap_or_default();
                let phonemes: Vec<&str> = words.iter().filter_map(|w| lexicon.get(w)).collect();
                phonemes.join(" ")
            } else {
                letters_word(part, lexicon, 0)
            }
        })
        .filter(|phonemes| !phonemes.is_empty())
        .collect();
    spoken.join(" ")
}

/// Splits `L42` into `L` and `42`, keeping ordinals and decimals whole (`21st`, `3.14`).
fn split_letters_and_digits(word: &str) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    for character in word.chars() {
        let digit_like = character.is_ascii_digit() || character == '.';
        let continues = parts.last().is_some_and(|part| {
            let part_is_number = part.starts_with(|c: char| c.is_ascii_digit());
            part_is_number == digit_like || (part_is_number && is_ordinal_tail(part, character))
        });
        match parts.last_mut() {
            Some(part) if continues => part.push(character),
            Some(_) | None => parts.push(character.to_string()),
        }
    }
    parts
}

fn is_ordinal_tail(part: &str, next: char) -> bool {
    let tail: String = part
        .chars()
        .skip_while(char::is_ascii_digit)
        .chain([next])
        .collect();
    ["s", "st", "n", "nd", "r", "rd", "t", "th"].contains(&tail.to_ascii_lowercase().as_str())
}

/// A word of letters (and apostrophes) through the lookup chain.
fn letters_word(word: &str, lexicon: &Lexicon, depth: u32) -> String {
    let upper = word.chars().all(|c| !c.is_alphabetic() || c.is_uppercase());
    if upper && word.chars().filter(|c| c.is_alphabetic()).count() > 1 {
        if let Some(phonemes) = lexicon.get_exact(word) {
            return phonemes.to_owned();
        }
        return spell(word, lexicon);
    }
    if let Some(phonemes) = lexicon
        .get(word)
        .or_else(|| lexicon.get(word.trim_end_matches('\'')))
    {
        return phonemes.to_owned();
    }
    if let Some(phonemes) = derived(word, lexicon) {
        return phonemes;
    }
    if depth < G2P_SPLIT_DEPTH_MAX
        && let Some(phonemes) = camel_case(word, lexicon, depth).or_else(|| compound(word, lexicon))
    {
        return phonemes;
    }
    rules::pronounce(word)
}

/// Letters spelled out with primary stress on the last one, as misaki does for unknown
/// acronyms: `API` → `ˌApˌiˈI`.
fn spell(word: &str, lexicon: &Lexicon) -> String {
    let letters: Vec<String> = word
        .chars()
        .filter(char::is_ascii_alphabetic)
        .filter_map(|letter| lexicon.get(&letter.to_ascii_uppercase().to_string()))
        .map(|phonemes| phonemes.replace('ˈ', "ˌ"))
        .collect();
    let mut joined = letters.concat();
    if let Some(index) = joined.rfind('ˌ') {
        joined.replace_range(index..index + 'ˌ'.len_utf8(), "ˈ");
    }
    joined
}

#[derive(Clone, Copy)]
enum Suffix {
    Plural,
    /// `-ies` from a stem ending in `y`, whose `i` sound is already there.
    PluralOfY,
    Past,
    Gerund,
}

/// Possessives, plurals, past tenses and gerunds of lexicon words, with misaki's suffix rules.
/// Candidates are tried in order; the first stem the lexicon knows wins.
fn derived(word: &str, lexicon: &Lexicon) -> Option<String> {
    let lower = word.to_lowercase();
    let mut candidates: Vec<(String, Suffix)> = Vec::new();
    if let Some(base) = lower.strip_suffix("'s") {
        candidates.push((base.to_owned(), Suffix::Plural));
    }
    if let Some(base) = lower.strip_suffix("ies") {
        candidates.push((format!("{base}y"), Suffix::PluralOfY));
    }
    if let Some(base) = lower
        .strip_suffix("es")
        .filter(|b| b.ends_with(['s', 'x', 'z', 'h']))
    {
        candidates.push((base.to_owned(), Suffix::Plural));
    }
    if let Some(base) = lower
        .strip_suffix('s')
        .filter(|b| !b.ends_with(['s', '\'']))
    {
        candidates.push((base.to_owned(), Suffix::Plural));
    }
    for (suffix, kind) in [("ed", Suffix::Past), ("ing", Suffix::Gerund)] {
        if let Some(base) = lower.strip_suffix(suffix) {
            candidates.push((base.to_owned(), kind));
            candidates.push((format!("{base}e"), kind));
            candidates.push((undouble(base), kind));
        }
    }
    candidates.into_iter().find_map(|(stem, kind)| {
        let phonemes = lexicon.get(&stem)?;
        Some(match kind {
            Suffix::Plural => plural(phonemes),
            Suffix::PluralOfY => format!("{phonemes}z"),
            Suffix::Past => past(phonemes),
            Suffix::Gerund => format!("{phonemes}ɪŋ"),
        })
    })
}

fn undouble(base: &str) -> String {
    let bytes = base.as_bytes();
    match bytes {
        [.., a, b] if a == b => base[..base.len() - 1].to_owned(),
        _ => base.to_owned(),
    }
}

fn last_sound(phonemes: &str) -> Option<char> {
    phonemes.chars().rev().find(|c| !matches!(c, 'ˈ' | 'ˌ'))
}

fn plural(phonemes: &str) -> String {
    match last_sound(phonemes) {
        Some('p' | 't' | 'k' | 'f' | 'θ') => format!("{phonemes}s"),
        Some('s' | 'z' | 'ʃ' | 'ʒ' | 'ʧ' | 'ʤ') => format!("{phonemes}ᵻz"),
        Some(_) | None => format!("{phonemes}z"),
    }
}

fn past(phonemes: &str) -> String {
    match last_sound(phonemes) {
        Some('t' | 'd') => format!("{phonemes}ᵻd"),
        Some('p' | 'k' | 'f' | 'θ' | 's' | 'ʃ' | 'ʧ') => format!("{phonemes}t"),
        Some(_) | None => format!("{phonemes}d"),
    }
}

/// `getUserById` → `get` `User` `By` `Id`; `OAuth` → `O` `Auth`.
fn camel_case(word: &str, lexicon: &Lexicon, depth: u32) -> Option<String> {
    let characters: Vec<char> = word.chars().collect();
    let mut parts: Vec<String> = Vec::new();
    for (index, character) in characters.iter().enumerate() {
        let next_is_lower = characters.get(index + 1).is_some_and(|c| c.is_lowercase());
        let previous_is_lower = index > 0 && characters[index - 1].is_lowercase();
        let boundary =
            index > 0 && character.is_uppercase() && (previous_is_lower || next_is_lower);
        if boundary || parts.is_empty() {
            parts.push(String::new());
        }
        if let Some(part) = parts.last_mut() {
            part.push(*character);
        }
    }
    (parts.len() > 1).then(|| {
        parts
            .iter()
            .map(|part| letters_word(part, lexicon, depth + 1))
            .collect::<Vec<_>>()
            .join(" ")
    })
}

/// Two known words written as one: `webhook` → `web` + `hook`. The longest known first part
/// wins; both parts must be at least three letters (or a known plural/past/gerund).
fn compound(word: &str, lexicon: &Lexicon) -> Option<String> {
    let lower = word.to_lowercase();
    if !lower.is_ascii() || lower.len() < 6 {
        return None;
    }
    (3..=lower.len() - 3).rev().find_map(|split| {
        let (first, second) = lower.split_at(split);
        let first = lexicon.get(first)?;
        let second = lexicon
            .get(second)
            .map(str::to_owned)
            .or_else(|| derived(second, lexicon))?;
        Some(format!("{first}{}", second.replace('ˈ', "ˌ")))
    })
}

#[cfg(test)]
mod tests {
    use super::{Lexicon, phonemize, word};
    use crate::speech_out::kokoro::vocab::token;

    fn lexicon() -> Lexicon {
        Lexicon::parse(
            r#"{"the":"ði","load":"lˈOd","balancer":"bˈælənsəɹ","pod":"pˈɑd","web":"wˈɛb",
                "hook":"hˈʊk","get":"ɡˈɛt","user":"jˈuzəɹ","by":"bˈI","id":"ˈɪd",
                "A":"ˈA","P":"pˈi","I":"ˈI","spread":"spɹˈɛd","cache":"kˈæʃ","bake":"bˈAk",
                "stop":"stˈɑp","run":"ɹˈʌn","forty":"fˈɔɹTi","two":"tˈu","point":"pˈYnt",
                "three":"θɹˈi","percent":"pəɹsˈɛnt","box":"bˈɑks","make":"mˈAk"}"#,
        )
        .expect("valid")
    }

    #[test]
    fn the_lookup_chain_covers_derived_forms() {
        let lexicon = lexicon();
        let cases = [
            ("pods", "pˈɑdz"),
            ("spreads", "spɹˈɛdz"),
            ("caches", "kˈæʃᵻz"),
            ("boxes", "bˈɑksᵻz"),
            ("baked", "bˈAkt"),
            ("stopped", "stˈɑpt"),
            ("running", "ɹˈʌnɪŋ"),
            ("making", "mˈAkɪŋ"),
            ("pod's", "pˈɑdz"),
            ("API", "ˌApˌiˈI"),
            ("webhook", "wˈɛbhˌʊk"),
            ("getUserById", "ɡˈɛt jˈuzəɹ bˈI ˈɪd"),
            ("42", "fˈɔɹTi tˈu"),
            ("3.3", "θɹˈi pˈYnt θɹˈi"),
        ];
        for (written, expected) in cases {
            assert_eq!(word(written, &lexicon), expected, "{written}");
        }
        // Not derivable: the rules answer.
        assert_eq!(word("redis", &lexicon), "ɹˈɛdɪs");
    }

    #[test]
    fn a_clause_keeps_word_offsets_and_punctuation() {
        let lexicon = lexicon();
        let text = "The load balancer, 42%!";
        let phonemized = phonemize(text, &lexicon);
        let offsets: Vec<u32> = phonemized.words.iter().map(|(offset, _)| *offset).collect();
        assert_eq!(offsets, vec![0, 4, 9, 19]);
        let symbols: String = phonemized
            .tokens
            .iter()
            .filter_map(|id| {
                (0..0x3000)
                    .filter_map(char::from_u32)
                    .find(|c| token(*c) == Some(*id))
            })
            .collect();
        assert_eq!(symbols, "ði lˈOd bˈælənsəɹ, fˈɔɹTi tˈu pəɹsˈɛnt!");
        // Each word's first token is the first symbol of its phonemes.
        let (_, balancer) = phonemized.words[2];
        assert_eq!(phonemized.tokens[balancer], token('b').unwrap_or_default());
    }
}
