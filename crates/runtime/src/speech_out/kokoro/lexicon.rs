//! misaki's American English "gold" lexicon (Apache-2.0): word → phonemes, already in Kokoro's
//! symbol set. Its sibling `us_silver` is left out on purpose: its author generated it by running
//! espeak-ng, whose GPL status for generated data is a question for the owner, not for code.
//!
//! Entries are either a phoneme string or a table by part of speech; without a tagger the
//! `DEFAULT` reading is used. Like misaki, a lowercase entry also answers its capitalized form
//! and the reverse. Kokoro v1.0 was trained with the flap as `T` and the glottal stop as `t`, so
//! those are mapped at load (misaki does the same for v1.0).

use std::collections::HashMap;
use std::path::Path;

use crate::limits::LEXICON_MAX_BYTES;

pub(crate) struct Lexicon {
    words: HashMap<String, String>,
}

impl Lexicon {
    pub(crate) fn load(path: &Path) -> Result<Self, String> {
        let size = std::fs::metadata(path)
            .map_err(|error| error.to_string())?
            .len();
        if size > LEXICON_MAX_BYTES {
            return Err(format!("the lexicon is {size} bytes, over its limit"));
        }
        let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
        Self::parse(&text)
    }

    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let entries: HashMap<String, serde_json::Value> = serde_json::from_str(text)
            .map_err(|error| format!("the lexicon is not valid: {error}"))?;
        let mut words = HashMap::with_capacity(entries.len() * 2);
        for (word, value) in &entries {
            let phonemes = match value {
                serde_json::Value::String(phonemes) => Some(phonemes.as_str()),
                serde_json::Value::Object(by_tag) => {
                    by_tag.get("DEFAULT").and_then(serde_json::Value::as_str)
                }
                serde_json::Value::Null
                | serde_json::Value::Bool(_)
                | serde_json::Value::Number(_)
                | serde_json::Value::Array(_) => None,
            };
            if let Some(phonemes) = phonemes {
                words.insert(word.clone(), for_kokoro_v1(phonemes));
            }
        }
        // misaki's `grow_dictionary`: lowercase entries answer for their capitalized form and
        // the reverse; ALL-CAPS entries (acronyms) stay as they are.
        let originals: Vec<(String, String)> = words
            .iter()
            .map(|(word, phonemes)| (word.clone(), phonemes.clone()))
            .collect();
        for (word, phonemes) in originals {
            let variant = if word == word.to_lowercase() {
                capitalized(&word)
            } else if word == capitalized(&word) {
                word.to_lowercase()
            } else {
                continue;
            };
            words.entry(variant).or_insert(phonemes);
        }
        Ok(Self { words })
    }

    /// The entry for `word` as written, else lowercase, else capitalized.
    pub(crate) fn get(&self, word: &str) -> Option<&str> {
        self.words
            .get(word)
            .or_else(|| self.words.get(&word.to_lowercase()))
            .or_else(|| self.words.get(&capitalized(word)))
            .map(String::as_str)
    }

    /// The entry for `word` exactly as written.
    pub(crate) fn get_exact(&self, word: &str) -> Option<&str> {
        self.words.get(word).map(String::as_str)
    }
}

fn capitalized(word: &str) -> String {
    let mut characters = word.chars();
    characters.next().map_or_else(String::new, |first| {
        first
            .to_uppercase()
            .chain(characters.flat_map(char::to_lowercase))
            .collect()
    })
}

fn for_kokoro_v1(phonemes: &str) -> String {
    phonemes.replace('ɾ', "T").replace('ʔ', "t")
}

#[cfg(test)]
mod tests {
    use super::Lexicon;

    #[test]
    fn entries_load_with_case_variants_and_default_readings() {
        let lexicon = Lexicon::parse(
            r#"{"hello":"həlˈO","SQL":"sˈikwəl","read":{"DEFAULT":"ɹˈid","VBD":"ɹˈɛd"},
                "ACT":{"DEFAULT":"ˈækt","NOUN":null},"butter":"bˈʌɾəɹ","gone":null}"#,
        )
        .expect("valid");
        assert_eq!(lexicon.get("hello"), Some("həlˈO"));
        assert_eq!(lexicon.get("Hello"), Some("həlˈO"));
        assert_eq!(lexicon.get("HELLO"), Some("həlˈO"));
        assert_eq!(lexicon.get("SQL"), Some("sˈikwəl"));
        assert_eq!(lexicon.get_exact("SQL"), Some("sˈikwəl"));
        assert_eq!(
            lexicon.get("sql"),
            None,
            "acronyms do not grow lowercase forms"
        );
        assert_eq!(lexicon.get("read"), Some("ɹˈid"));
        assert_eq!(lexicon.get("ACT"), Some("ˈækt"));
        assert_eq!(lexicon.get("butter"), Some("bˈʌTəɹ"));
        assert_eq!(lexicon.get("gone"), None);
        assert_eq!(lexicon.get_exact("Sql"), None);
        assert!(Lexicon::parse("[1]").is_err());
    }
}
