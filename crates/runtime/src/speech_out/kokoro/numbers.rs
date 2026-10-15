//! Numbers as English words, so the lexicon can pronounce them. A short function instead of
//! `num2words`, which is LGPL. Integers, decimals, ordinals (`1st`) and four-digit years.

use crate::limits::NUMBER_DIGITS_MAX;

const ONES: [&str; 20] = [
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
];
const TENS: [&str; 10] = [
    "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];
const SCALES: [(u64, &str); 4] = [
    (1_000_000_000_000, "trillion"),
    (1_000_000_000, "billion"),
    (1_000_000, "million"),
    (1_000, "thousand"),
];

/// The words for `0..1000`, appended to `out`.
fn below_thousand(value: u64, out: &mut Vec<&'static str>) {
    let hundreds = value / 100;
    let rest = value % 100;
    if hundreds > 0 {
        out.push(ONES[hundreds as usize]);
        out.push("hundred");
    }
    if rest >= 20 {
        out.push(TENS[(rest / 10) as usize]);
        if !rest.is_multiple_of(10) {
            out.push(ONES[(rest % 10) as usize]);
        }
    } else if rest > 0 || hundreds == 0 {
        out.push(ONES[rest as usize]);
    }
}

/// An integer in words: `1205` → `one thousand two hundred five`.
#[must_use]
pub(crate) fn integer(value: u64) -> Vec<&'static str> {
    let mut out = Vec::new();
    let mut rest = value;
    for (scale, name) in SCALES {
        if rest >= scale {
            below_thousand(rest / scale, &mut out);
            out.push(name);
            rest %= scale;
        }
    }
    if rest > 0 || out.is_empty() {
        below_thousand(rest, &mut out);
    }
    out
}

/// A year the way it is said: `1999` → `nineteen ninety nine`, `2026` → `twenty twenty six`,
/// `2005` → `two thousand five`, `1900` → `nineteen hundred`.
fn year(value: u64) -> Vec<&'static str> {
    let (high, low) = (value / 100, value % 100);
    if (2_000..2_010).contains(&value) {
        return integer(value);
    }
    let mut out = Vec::new();
    below_thousand(high, &mut out);
    match low {
        0 => out.push("hundred"),
        1..=9 => {
            out.push("O");
            out.push(ONES[low as usize]);
        }
        _ => below_thousand(low, &mut out),
    }
    out
}

/// The last word made ordinal: `two` → `second`, `twenty` → `twentieth`.
fn ordinal(mut words: Vec<&'static str>) -> Vec<String> {
    let last = words.pop().unwrap_or("zero");
    let suffixed = match last {
        "one" => "first".to_owned(),
        "two" => "second".to_owned(),
        "three" => "third".to_owned(),
        "five" => "fifth".to_owned(),
        "eight" => "eighth".to_owned(),
        "nine" => "ninth".to_owned(),
        "twelve" => "twelfth".to_owned(),
        other if other.ends_with('y') => format!("{}ieth", &other[..other.len() - 1]),
        other => format!("{other}th"),
    };
    let mut out: Vec<String> = words.into_iter().map(str::to_owned).collect();
    out.push(suffixed);
    out
}

/// Words for a numeric token: digits with an optional decimal part (`3.14`) or ordinal suffix
/// (`21st`). `None` when the token is not a number this reads.
#[must_use]
pub(crate) fn words(token: &str) -> Option<Vec<String>> {
    let owned = |words: Vec<&'static str>| words.into_iter().map(str::to_owned).collect();
    let digits_end = token
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(token.len());
    let (digits, rest) = token.split_at(digits_end);
    if digits.is_empty() {
        return None;
    }
    if digits.len() > NUMBER_DIGITS_MAX || (digits.len() > 1 && digits.starts_with('0')) {
        return rest
            .is_empty()
            .then(|| digits.chars().map(digit).map(str::to_owned).collect());
    }
    let value: u64 = digits.parse().ok()?;
    match rest.to_ascii_lowercase().as_str() {
        "" if digits.len() == 4 && (1_100..2_100).contains(&value) => Some(owned(year(value))),
        "" => Some(owned(integer(value))),
        "st" | "nd" | "rd" | "th" => Some(ordinal(integer(value))),
        fraction if fraction.starts_with('.') && fraction.len() > 1 => {
            let decimals = &fraction[1..];
            if !decimals.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            let mut out: Vec<String> = owned(integer(value));
            out.push("point".to_owned());
            out.extend(decimals.chars().map(digit).map(str::to_owned));
            Some(out)
        }
        _ => None,
    }
}

fn digit(character: char) -> &'static str {
    character
        .to_digit(10)
        .map_or("zero", |value| ONES[value as usize])
}

#[cfg(test)]
mod tests {
    use super::words;

    fn say(token: &str) -> Option<String> {
        words(token).map(|words| words.join(" "))
    }

    #[test]
    fn numbers_read_as_people_say_them() {
        let cases = [
            ("0", "zero"),
            ("7", "seven"),
            ("42", "forty two"),
            ("100", "one hundred"),
            ("12050", "twelve thousand fifty"),
            ("1205", "twelve O five"),
            ("3000000", "three million"),
            ("1999", "nineteen ninety nine"),
            ("2026", "twenty twenty six"),
            ("2005", "two thousand five"),
            ("1900", "nineteen hundred"),
            ("1905", "nineteen O five"),
            ("1st", "first"),
            ("22nd", "twenty second"),
            ("40th", "fortieth"),
            ("3.14", "three point one four"),
            ("007", "zero zero seven"),
        ];
        for (token, expected) in cases {
            assert_eq!(say(token).as_deref(), Some(expected), "{token}");
        }
        assert_eq!(say("abc"), None);
        assert_eq!(say("12px"), None);
    }
}
