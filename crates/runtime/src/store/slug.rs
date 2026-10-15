//! Safe file names from titles: lowercase ASCII letters, digits and single dashes, bounded, so
//! a slug is always one path component and never `.`, `..` or hidden.

use crate::limits::{VAULT_NAME_COLLISIONS_MAX, VAULT_SLUG_MAX_BYTES};

/// The slug of `text`, or `None` when nothing usable is left (empty, or only symbols).
/// Common accented Latin letters fold to ASCII (`café` → `cafe`); other letters separate
/// words. A slug longer than [`VAULT_SLUG_MAX_BYTES`] is cut at its last dash before the limit.
#[must_use]
pub fn slugify(text: &str) -> Option<String> {
    let mut slug = String::new();
    let mut pending_dash = false;
    for character in text.chars() {
        if slug.len() > VAULT_SLUG_MAX_BYTES {
            break;
        }
        if character.is_ascii_alphanumeric() {
            push_letter(&mut slug, &mut pending_dash, character.to_ascii_lowercase());
        } else if let Some(ascii) = fold(character) {
            for letter in ascii.chars() {
                push_letter(&mut slug, &mut pending_dash, letter);
            }
        } else {
            pending_dash = true;
        }
    }
    if slug.len() > VAULT_SLUG_MAX_BYTES {
        let cut = &slug[..VAULT_SLUG_MAX_BYTES];
        let end = cut.rfind('-').filter(|&at| at > 0).unwrap_or(cut.len());
        slug.truncate(end);
    }
    let trimmed = slug.trim_matches('-');
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn push_letter(slug: &mut String, pending_dash: &mut bool, letter: char) {
    if *pending_dash && !slug.is_empty() {
        slug.push('-');
    }
    *pending_dash = false;
    slug.push(letter);
}

/// The first of `base`, `base-2`, `base-3`… that `is_taken` rejects, or `None` past
/// [`VAULT_NAME_COLLISIONS_MAX`]. Deterministic: the same taken names give the same answer.
pub fn first_free(base: &str, is_taken: impl Fn(&str) -> bool) -> Option<String> {
    if !is_taken(base) {
        return Some(base.to_owned());
    }
    (2..=VAULT_NAME_COLLISIONS_MAX)
        .map(|number| format!("{base}-{number}"))
        .find(|candidate| !is_taken(candidate))
}

/// Common accented Latin letters, folded to ASCII for names in titles. Anything else that is
/// not an ASCII letter or digit separates words.
fn fold(character: char) -> Option<&'static str> {
    const TABLE: &[(&str, &str)] = &[
        ("àáâãäåāÀÁÂÃÄÅĀ", "a"),
        ("æÆ", "ae"),
        ("çćčÇĆČ", "c"),
        ("ďđĎĐ", "d"),
        ("èéêëēěęÈÉÊËĒĚĘ", "e"),
        ("ìíîïīÌÍÎÏĪ", "i"),
        ("łŁ", "l"),
        ("ñńňÑŃŇ", "n"),
        ("òóôõöøōÒÓÔÕÖØŌ", "o"),
        ("œŒ", "oe"),
        ("řŕŘŔ", "r"),
        ("śšşŚŠŞ", "s"),
        ("ß", "ss"),
        ("ťţŤŢ", "t"),
        ("ùúûüūůÙÚÛÜŪŮ", "u"),
        ("ýÿÝŸ", "y"),
        ("źżžŹŻŽ", "z"),
    ];
    TABLE
        .iter()
        .find(|(letters, _)| letters.contains(character))
        .map(|(_, ascii)| *ascii)
}

#[cfg(test)]
mod tests {
    use super::{first_free, slugify};
    use crate::limits::VAULT_SLUG_MAX_BYTES;

    #[test]
    fn slugs_are_safe_single_components() {
        for (text, slug) in [
            (
                "How a request reaches the API",
                Some("how-a-request-reaches-the-api"),
            ),
            ("  Redis: TTLs & eviction!! ", Some("redis-ttls-eviction")),
            ("Café crème", Some("cafe-creme")),
            ("../../etc/passwd", Some("etc-passwd")),
            ("C:\\Windows\\..", Some("c-windows")),
            (".hidden", Some("hidden")),
            ("日本語", None),
            ("", None),
            ("---", None),
            ("Straße", Some("strasse")),
        ] {
            assert_eq!(slugify(text).as_deref(), slug, "{text:?}");
        }
    }

    #[test]
    fn long_slugs_are_cut_at_a_word() {
        let long = "word ".repeat(40);
        let slug = slugify(&long).unwrap_or_default();
        assert!(slug.len() <= VAULT_SLUG_MAX_BYTES);
        assert!(slug.ends_with("word") && !slug.ends_with('-'));
        let one_word = "x".repeat(200);
        assert_eq!(
            slugify(&one_word).map(|slug| slug.len()),
            Some(VAULT_SLUG_MAX_BYTES)
        );
    }

    #[test]
    fn collisions_get_deterministic_numbers() {
        let taken = ["notes", "notes-2", "notes-3"];
        assert_eq!(
            first_free("notes", |name| taken.contains(&name)).as_deref(),
            Some("notes-4")
        );
        assert_eq!(first_free("free", |_| false).as_deref(), Some("free"));
        assert_eq!(first_free("full", |_| true), None);
    }
}
