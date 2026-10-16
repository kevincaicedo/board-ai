//! Speakable text (LLD §2.3). Captions show text as written; text-to-speech gets a normalized
//! form: `getUserByID` → "get user by ID", `service.go` → "service dot go", `L42` → "line 42".
//!
//! Normalization is table-driven and records, piece by piece, which caption bytes produced which
//! speakable bytes. Word timings arrive in speakable offsets; the map turns them back into caption
//! offsets for the caption reveal and the heard prefix, and turns op anchors (caption offsets)
//! into speakable offsets for scheduling.

/// Whole tokens with a spoken form, matched case-insensitively.
const WHOLE_TOKENS: &[(&str, &str)] = &[
    ("e.g.", "for example"),
    ("i.e.", "that is"),
    ("etc.", "et cetera"),
    ("vs.", "versus"),
    ("vs", "versus"),
    ("w/", "with"),
    ("->", "to"),
    ("=>", "to"),
    ("→", "to"),
    ("←", "from"),
    ("&&", "and"),
    ("||", "or"),
    ("==", "equals"),
    ("!=", "is not"),
    (">=", "at least"),
    ("<=", "at most"),
    ("&", "and"),
    ("+", "plus"),
    ("=", "equals"),
    ("%", "percent"),
    ("×", "times"),
    ("~", "about"),
    ("≈", "about"),
    // Markdown list and heading marks are layout, never speech (the prompt asks for none, and
    // models still write them).
    ("-", ""),
    ("•", ""),
    ("#", ""),
    ("##", ""),
    ("###", ""),
];

/// Arrows written inside a token ("client->CDN", "API→Redis"), between word characters, read as
/// "to". Longest first, so "-->" is not read as "-" plus "->".
const ARROWS: &[&str] = &["-->", "->", "=>", "→", "⇒"];

/// Separators inside a token, between word characters. `.` between two digits stays a decimal
/// point; `::` is handled before these.
const SEPARATORS: &[(char, &str)] = &[('.', " dot "), ('/', " slash "), ('_', " ")];

/// Characters that are formatting, never spoken, at the edges of a token: Markdown code and
/// emphasis marks (`*hot path*`, `**Redis**`, `_cache_`). Inside a token `_` stays a separator
/// (`user_id`).
const UNSPOKEN: &[char] = &['`', '*', '_'];

/// Punctuation kept as written around a token; text-to-speech uses it for prosody.
const LEADING_PUNCTUATION: &[char] = &['(', '[', '"', '\'', '“', '‘'];
const TRAILING_PUNCTUATION: &[char] = &[
    '.', ',', ';', ':', '!', '?', ')', ']', '"', '\'', '”', '’', '…',
];

/// One mapped span: `caption[caption_begin..caption_end]` became
/// `speakable[speakable_begin..speakable_end]`. Spans are contiguous and in order on both sides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Span {
    caption_begin: usize,
    caption_end: usize,
    speakable_begin: usize,
    speakable_end: usize,
}

impl Span {
    /// Same length on both sides: offsets map one to one (case changes keep the length).
    fn is_linear(self) -> bool {
        self.caption_end - self.caption_begin == self.speakable_end - self.speakable_begin
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Speakable {
    text: String,
    caption_len: usize,
    spans: Vec<Span>,
}

impl Speakable {
    #[must_use]
    pub fn from_caption(caption: &str) -> Self {
        let mut builder = Builder {
            caption,
            text: String::with_capacity(caption.len() + caption.len() / 4),
            spans: Vec::new(),
        };
        let mut token_begin: Option<usize> = None;
        for (index, ch) in caption.char_indices() {
            match (ch.is_whitespace(), token_begin) {
                (true, Some(begin)) => {
                    builder.token(begin, index);
                    token_begin = None;
                    builder.copy(index, index + ch.len_utf8());
                }
                (true, None) => builder.copy(index, index + ch.len_utf8()),
                (false, None) => token_begin = Some(index),
                (false, Some(_)) => {}
            }
        }
        if let Some(begin) = token_begin {
            builder.token(begin, caption.len());
        }
        Self {
            text: builder.text,
            caption_len: caption.len(),
            spans: builder.spans,
        }
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The caption offset that a speakable offset corresponds to. Inside a rewritten span it is
    /// the span's start, so a half-spoken `getUserByID` never counts as heard beyond what was.
    #[must_use]
    pub fn caption_offset(&self, speakable_offset: usize) -> usize {
        if speakable_offset >= self.text.len() {
            return self.caption_len;
        }
        let Some(span) = self.spans.iter().find(|span| {
            span.speakable_begin <= speakable_offset && speakable_offset < span.speakable_end
        }) else {
            return self.caption_len;
        };
        if span.is_linear() {
            span.caption_begin + (speakable_offset - span.speakable_begin)
        } else {
            span.caption_begin
        }
    }

    /// The speakable offset where a caption offset is spoken.
    #[must_use]
    pub fn speakable_offset(&self, caption_offset: usize) -> usize {
        if caption_offset >= self.caption_len {
            return self.text.len();
        }
        let Some(span) = self
            .spans
            .iter()
            .find(|span| span.caption_begin <= caption_offset && caption_offset < span.caption_end)
        else {
            return self.text.len();
        };
        if span.is_linear() {
            span.speakable_begin + (caption_offset - span.caption_begin)
        } else {
            span.speakable_begin
        }
    }
}

struct Builder<'a> {
    caption: &'a str,
    text: String,
    spans: Vec<Span>,
}

impl Builder<'_> {
    fn emit(&mut self, caption_begin: usize, caption_end: usize, spoken: &str) {
        let speakable_begin = self.text.len();
        self.text.push_str(spoken);
        self.spans.push(Span {
            caption_begin,
            caption_end,
            speakable_begin,
            speakable_end: self.text.len(),
        });
    }

    fn copy(&mut self, begin: usize, end: usize) {
        let caption = self.caption;
        self.emit(begin, end, &caption[begin..end]);
    }

    /// One whitespace-free token: punctuation around it, then its core.
    fn token(&mut self, begin: usize, end: usize) {
        let token = &self.caption[begin..end];
        if let Some(spoken) = whole_token(token) {
            self.emit(begin, end, spoken);
            return;
        }
        let mut core_begin = begin;
        for (offset, ch) in token.char_indices() {
            if !LEADING_PUNCTUATION.contains(&ch) && !UNSPOKEN.contains(&ch) {
                break;
            }
            core_begin = begin + offset + ch.len_utf8();
        }
        let mut core_end = end;
        for (offset, ch) in token.char_indices().rev() {
            let call_parens = ch == ')' && token[..offset].ends_with('(');
            if begin + offset < core_begin
                || call_parens
                || (!TRAILING_PUNCTUATION.contains(&ch) && !UNSPOKEN.contains(&ch))
            {
                break;
            }
            core_end = begin + offset;
        }
        let call_parens = self.caption[core_begin..core_end].ends_with("()");
        let word_end = if call_parens { core_end - 2 } else { core_end };
        self.punctuation(begin, core_begin);
        self.core(core_begin, word_end);
        if call_parens {
            self.emit(word_end, core_end, "");
        }
        self.punctuation(core_end, end);
    }

    fn punctuation(&mut self, begin: usize, end: usize) {
        let caption = self.caption;
        for (offset, ch) in caption[begin..end].char_indices() {
            let at = begin + offset;
            if UNSPOKEN.contains(&ch) {
                self.emit(at, at + ch.len_utf8(), "");
            } else {
                self.copy(at, at + ch.len_utf8());
            }
        }
    }

    fn core(&mut self, begin: usize, end: usize) {
        if begin >= end {
            return;
        }
        let core = &self.caption[begin..end];
        if let Some(spoken) = whole_token(core) {
            self.emit(begin, end, spoken);
            return;
        }
        if self.line_reference(begin, end) {
            return;
        }
        let screaming = core.contains('_') && !core.chars().any(char::is_lowercase);
        let mut part_begin = begin;
        let mut index = begin;
        while index < end {
            let Some(ch) = self.caption[index..end].chars().next() else {
                break;
            };
            if let Some((length, spoken)) = self.separator_at(index, begin, end) {
                self.word(part_begin, index, screaming);
                self.emit(index, index + length, spoken);
                index += length;
                part_begin = index;
            } else {
                index += ch.len_utf8();
            }
        }
        self.word(part_begin, end, screaming);
    }

    /// A separator starting at `index`, with its byte length and spoken form.
    fn separator_at(
        &self,
        index: usize,
        begin: usize,
        end: usize,
    ) -> Option<(usize, &'static str)> {
        let rest = &self.caption[index..end];
        let previous = self.caption[begin..index].chars().next_back()?;
        if rest.starts_with("::") && index + 2 < end {
            return Some((2, " "));
        }
        if let Some(arrow) = ARROWS.iter().find(|arrow| rest.starts_with(**arrow)) {
            let after = rest[arrow.len()..].chars().next()?;
            return (previous.is_alphanumeric() && after.is_alphanumeric())
                .then_some((arrow.len(), " to "));
        }
        let mut chars = rest.chars();
        let ch = chars.next()?;
        let next = chars.next()?;
        if ch == ':' && rest[1..].bytes().all(|byte| byte.is_ascii_digit()) {
            return Some((1, " line "));
        }
        if ch == '.' && previous.is_ascii_digit() && next.is_ascii_digit() {
            return None;
        }
        let (_, spoken) = SEPARATORS.iter().find(|(separator, _)| *separator == ch)?;
        (previous.is_alphanumeric() && next.is_alphanumeric()).then_some((1, *spoken))
    }

    /// `L42` → "line 42", `L42-58` → "lines 42 to 58".
    fn line_reference(&mut self, begin: usize, end: usize) -> bool {
        let core = &self.caption[begin..end];
        let Some(numbers) = core.strip_prefix('L') else {
            return false;
        };
        let (first, last) = match numbers.split_once(['-', '–']) {
            Some((first, last)) => (first, Some(last)),
            None => (numbers, None),
        };
        let digits =
            |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
        if !digits(first) || !last.is_none_or(digits) {
            return false;
        }
        let first_begin = begin + 1;
        let first_end = first_begin + first.len();
        self.emit(
            begin,
            first_begin,
            if last.is_some() { "lines " } else { "line " },
        );
        self.copy(first_begin, first_end);
        if last.is_some() {
            let dash_end = first_end
                + self.caption[first_end..]
                    .chars()
                    .next()
                    .map_or(1, char::len_utf8);
            self.emit(first_end, dash_end, " to ");
            self.copy(dash_end, end);
        }
        true
    }

    /// A word with no separators: camelCase is split, and words are lowercased except acronyms.
    fn word(&mut self, begin: usize, end: usize, screaming: bool) {
        if begin >= end {
            return;
        }
        let word = &self.caption[begin..end];
        let parts = camel_parts(word);
        if parts.len() == 1 && !screaming {
            self.copy(begin, end);
            return;
        }
        for (index, (part_begin, part_end)) in parts.iter().enumerate() {
            let at_begin = begin + part_begin;
            let at_end = begin + part_end;
            if index > 0 {
                self.emit(at_begin, at_begin, " ");
            }
            let part = &word[*part_begin..*part_end];
            let acronym = !screaming && part.len() >= 2 && !part.chars().any(char::is_lowercase);
            if acronym {
                self.copy(at_begin, at_end);
            } else {
                self.emit(at_begin, at_end, &part.to_lowercase());
            }
        }
    }
}

fn whole_token(token: &str) -> Option<&'static str> {
    WHOLE_TOKENS
        .iter()
        .find(|(written, _)| written.eq_ignore_ascii_case(token))
        .map(|(_, spoken)| *spoken)
}

/// Byte ranges of camelCase parts: a split before an uppercase letter that follows a lowercase
/// one, and before the last capital of an acronym followed by a lowercase word ("HTTPServer" →
/// "HTTP", "Server"). Plural acronyms ("APIs", "IDs") and plain words ("Redis") stay whole.
fn camel_parts(word: &str) -> Vec<(usize, usize)> {
    let chars: Vec<(usize, char)> = word.char_indices().collect();
    let lower_at = |index: usize| chars.get(index).is_some_and(|(_, ch)| ch.is_lowercase());
    let mut parts = Vec::new();
    let mut part_begin = 0;
    for index in 1..chars.len() {
        let (offset, ch) = chars[index];
        let previous = chars[index - 1].1;
        let hump = ch.is_uppercase()
            && (previous.is_lowercase()
                || (previous.is_uppercase() && lower_at(index + 1) && lower_at(index + 2)));
        if hump {
            parts.push((part_begin, offset));
            part_begin = offset;
        }
    }
    parts.push((part_begin, word.len()));
    parts
}

#[cfg(test)]
mod tests {
    use super::{SEPARATORS, Speakable, WHOLE_TOKENS};

    #[test]
    fn normalizes_code_words_for_speech() {
        for (caption, spoken) in [
            ("getUserByID", "get user by ID"),
            ("Call getUserByID() first.", "Call get user by ID first."),
            ("service.go", "service dot go"),
            (
                "internal/user/service.go",
                "internal slash user slash service dot go",
            ),
            ("see L42", "see line 42"),
            ("L42-58,", "lines 42 to 58,"),
            ("service.go:42", "service dot go line 42"),
            ("user_id", "user id"),
            ("MAX_RETRIES", "max retries"),
            ("HTTPServer", "HTTP server"),
            ("the APIs and IDs", "the APIs and IDs"),
            ("std::sync", "std sync"),
            ("`redis`", "redis"),
            ("Redis and the API", "Redis and the API"),
            ("3.5 seconds", "3.5 seconds"),
            ("e.g. Kafka", "for example Kafka"),
            ("a -> b", "a to b"),
            ("client->CDN", "client to CDN"),
            ("API→Redis→Postgres.", "API to Redis to Postgres."),
            ("lb=>api", "lb to api"),
            ("a-->b", "a to b"),
            ("Client → Edge → LB", "Client to Edge to LB"),
            ("the *hot path* first", "the hot path first"),
            ("**Redis**, then _Postgres_.", "Redis, then Postgres."),
            ("__init__ runs", "init runs"),
            ("- First the client.", " First the client."),
            ("• Then the CDN", " Then the CDN"),
            ("## Step one", " Step one"),
            ("write-through and read-only", "write-through and read-only"),
            ("x - y", "x  y"),
        ] {
            assert_eq!(Speakable::from_caption(caption).text(), spoken, "{caption}");
        }
    }

    #[test]
    fn every_table_entry_is_spoken() {
        for (written, spoken) in WHOLE_TOKENS {
            assert_eq!(
                Speakable::from_caption(written).text(),
                *spoken,
                "{written}"
            );
        }
        for (separator, spoken) in SEPARATORS {
            let caption = format!("a{separator}b");
            assert_eq!(
                Speakable::from_caption(&caption).text(),
                format!("a{spoken}b")
            );
        }
    }

    #[test]
    fn offsets_map_both_ways() {
        let speakable = Speakable::from_caption("It calls getUserByID in service.go now");
        let text = speakable.text();
        assert_eq!(text, "It calls get user by ID in service dot go now");
        let user = text.find("user").expect("present");
        // "user" comes from "User" (same length): linear.
        assert_eq!(speakable.caption_offset(user), "It calls get".len());
        let dot = text.find("dot").expect("present");
        // Inside the rewritten "." the caption offset snaps to the span start.
        assert_eq!(
            speakable.caption_offset(dot + 1),
            "It calls getUserByID in service".len()
        );
        let now_caption = "It calls getUserByID in service.go ".len();
        assert_eq!(
            speakable.speakable_offset(now_caption),
            text.find("now").expect("present")
        );
        assert_eq!(speakable.caption_offset(text.len()), 38);
        let mut previous = 0;
        for offset in 0..=text.len() {
            let caption = speakable.caption_offset(offset);
            assert!(
                caption >= previous && caption <= 38,
                "monotonic at {offset}"
            );
            previous = caption;
        }
    }
}
