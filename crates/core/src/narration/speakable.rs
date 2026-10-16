//! Speakable text (LLD §2.3). Captions show text as written; text-to-speech gets a normalized
//! form: `getUserByID` → "get user by ID", `service.go` → "service dot go", `L42` → "line 42",
//! `tcp.go:88-95` → "tcp dot go, lines 88 to 95", `12:30` → "12 30".
//!
//! Normalization is table-driven and records, piece by piece, which caption bytes produced which
//! speakable bytes. Word timings arrive in speakable offsets; the map turns them back into caption
//! offsets for the caption reveal and the heard prefix, and turns op anchors (caption offsets)
//! into speakable offsets for scheduling.

use super::citation;

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

/// How a line reference reads before its numbers: `(one line, a range of lines)`. Alone (`L42`),
/// or after a cited path (`service.go:42`), where the comma gives the path its pause.
const LINE_WORDS: (&str, &str) = ("line ", "lines ");
const CITED_LINE_WORDS: (&str, &str) = (", line ", ", lines ");
/// Between a range's ends: `L42-58` → "lines 42 to 58".
const LINE_RANGE_TO: &str = " to ";

/// A clock time's minutes after the hour: `9:00` → "9 o'clock", `15:00` → "15 hundred",
/// `9:05` → "9 oh 5", `12:30` → "12 30". On the hour before a meridiem (`7:00 pm`) they are
/// silent: "7 P M".
const ON_THE_HOUR: &str = " o'clock";
const ON_THE_HOUR_24: &str = " hundred";
const BEFORE_TEN_PAST: &str = " oh ";
const MINUTES: &str = " ";
/// A clock time's meridiem, written onto it (`7:30pm`) or as the next word (`7:30 PM`), matched
/// case-insensitively without a final full stop. Spelled as letters: misaki's lexicon (Kokoro)
/// reads `AM` as the verb "am".
const MERIDIEMS: &[(&str, &str)] = &[("am", "A M"), ("a.m", "A M"), ("pm", "P M"), ("p.m", "P M")];

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
            meridiem_at: None,
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
    /// Where a meridiem word begins that the clock time before it claimed (`9:00 PM`).
    meridiem_at: Option<usize>,
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
        if self.meridiem_at.take_if(|at| *at == begin).is_some()
            && let Some(spoken) = meridiem(core)
        {
            self.emit(begin, end, spoken);
            return;
        }
        if self.line_reference(begin, end)
            || self.cited_lines(begin, end)
            || self.clock_time(begin, end)
        {
            return;
        }
        self.separated(begin, end);
    }

    /// A token cut at its separators (`.`, `/`, `_`, `::`, arrows), each part read as a word.
    fn separated(&mut self, begin: usize, end: usize) {
        let core = &self.caption[begin..end];
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
        if ch == '.' && previous.is_ascii_digit() && next.is_ascii_digit() {
            return None;
        }
        let (_, spoken) = SEPARATORS.iter().find(|(separator, _)| *separator == ch)?;
        (previous.is_alphanumeric() && next.is_alphanumeric()).then_some((1, *spoken))
    }

    /// `L42` → "line 42", `L42-58` → "lines 42 to 58".
    fn line_reference(&mut self, begin: usize, end: usize) -> bool {
        if !self.caption[begin..end].starts_with('L') {
            return false;
        }
        let Some(numbers) = LineNumbers::parse(&self.caption[begin + 1..end]) else {
            return false;
        };
        self.lines(begin, begin + 1, numbers, LINE_WORDS);
        true
    }

    /// A citation's lines after its path, by the same rule that makes it a citation (a path with
    /// a file extension or a folder): `tcp.go:88-95` → "tcp dot go, lines 88 to 95",
    /// `handler.go:L12` → "handler dot go, line 12". Any other `word:42` keeps its colon.
    fn cited_lines(&mut self, begin: usize, end: usize) -> bool {
        let core = &self.caption[begin..end];
        let Some(colon) = core.rfind(':') else {
            return false;
        };
        let lead = if core[colon + 1..].starts_with('L') {
            2
        } else {
            1
        };
        let Some(numbers) = LineNumbers::parse(&core[colon + lead..]) else {
            return false;
        };
        if citation::parse(core).is_none() {
            return false;
        }
        self.separated(begin, begin + colon);
        self.lines(
            begin + colon,
            begin + colon + lead,
            numbers,
            CITED_LINE_WORDS,
        );
        true
    }

    /// `lead` is `begin..numbers_begin`: the `L` of `L42`, or the `:` or `:L` after a path.
    fn lines(
        &mut self,
        begin: usize,
        numbers_begin: usize,
        numbers: LineNumbers,
        (one, range): (&str, &str),
    ) {
        let first_end = numbers_begin + numbers.first_size;
        self.emit(
            begin,
            numbers_begin,
            if numbers.last_begin.is_some() {
                range
            } else {
                one
            },
        );
        self.copy(numbers_begin, first_end);
        if let Some(last_begin) = numbers.last_begin {
            let last_begin = numbers_begin + last_begin;
            self.emit(first_end, last_begin, LINE_RANGE_TO);
            self.copy(last_begin, numbers_begin + numbers.size);
        }
    }

    /// A clock time, `12:30` → "12 30", `9:05` → "9 oh 5", `09:00` → "9 o'clock",
    /// `7:00 pm` → "7 P M". Never a line: that needs a path (`cited_lines`).
    fn clock_time(&mut self, begin: usize, end: usize) -> bool {
        let Some(mut time) = ClockTime::parse(&self.caption[begin..end]) else {
            return false;
        };
        if time.meridiem == Meridiem::Absent
            && (1..=12).contains(&time.hour)
            && self.claim_next_meridiem(end)
        {
            time.meridiem = Meridiem::NextWord;
        }
        let hour_end = begin + time.hour_size;
        let minutes_begin = hour_end + 1;
        let minutes_end = minutes_begin + 2;
        // A leading zero is silent ("09:30" is "9 30"), as people say it.
        let hour_begin = if time.hour_size == 2 && self.caption[begin..].starts_with('0') {
            self.emit(begin, begin + 1, "");
            begin + 1
        } else {
            begin
        };
        self.copy(hour_begin, hour_end);
        match (time.minutes, time.meridiem) {
            (0, Meridiem::Attached(_) | Meridiem::NextWord) => {
                self.emit(hour_end, minutes_end, "");
            }
            (0, Meridiem::Absent) if (1..=12).contains(&time.hour) => {
                self.emit(hour_end, minutes_end, ON_THE_HOUR);
            }
            (0, Meridiem::Absent) => self.emit(hour_end, minutes_end, ON_THE_HOUR_24),
            (1..=9, _) => {
                self.emit(hour_end, minutes_begin + 1, BEFORE_TEN_PAST);
                self.copy(minutes_begin + 1, minutes_end);
            }
            (_, _) => {
                self.emit(hour_end, minutes_begin, MINUTES);
                self.copy(minutes_begin, minutes_end);
            }
        }
        match time.meridiem {
            Meridiem::Attached(spoken) => {
                self.emit(minutes_end, minutes_end, " ");
                self.emit(minutes_end, end, spoken);
            }
            Meridiem::Absent | Meridiem::NextWord => {}
        }
        true
    }

    /// A meridiem as the word right after a clock time (`9:00 PM`, `7:30 a.m.`): `core` speaks
    /// it when it reaches it.
    fn claim_next_meridiem(&mut self, time_end: usize) -> bool {
        let Some(next) = self.caption[time_end..].strip_prefix(' ') else {
            return false;
        };
        let word = next
            .split(char::is_whitespace)
            .next()
            .unwrap_or_default()
            .trim_end_matches(TRAILING_PUNCTUATION);
        let claimed = meridiem(word).is_some();
        if claimed {
            self.meridiem_at = Some(time_end + 1);
        }
        claimed
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

fn meridiem(written: &str) -> Option<&'static str> {
    MERIDIEMS
        .iter()
        .find(|(meridiem, _)| meridiem.eq_ignore_ascii_case(written))
        .map(|(_, spoken)| *spoken)
}

/// A line reference's numbers, `42` or `42-58` (an en dash too). Offsets are bytes from the
/// first digit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LineNumbers {
    first_size: usize,
    /// Where a range's last number begins, past its dash.
    last_begin: Option<usize>,
    size: usize,
}

impl LineNumbers {
    fn parse(text: &str) -> Option<Self> {
        let digits =
            |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
        let (first, last_begin) = match text.split_once(['-', '–']) {
            Some((first, last)) => (first, Some(text.len() - last.len())),
            None => (text, None),
        };
        let last_valid = last_begin.is_none_or(|begin| digits(&text[begin..]));
        (digits(first) && last_valid).then_some(Self {
            first_size: first.len(),
            last_begin,
            size: text.len(),
        })
    }
}

/// `H:MM` with an hour to 23 (one or two digits), minutes to 59 (two), and an optional meridiem
/// written onto it (`7:30pm`, hour 1 to 12). `16:9`, `3:1` and `12:30:45` are not clock times.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ClockTime {
    hour: u32,
    hour_size: usize,
    minutes: u32,
    meridiem: Meridiem,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Meridiem {
    Absent,
    /// Written onto the time (`7:30pm`), with its spoken form.
    Attached(&'static str),
    /// The word after the time (`7:30 PM`), which speaks itself.
    NextWord,
}

impl ClockTime {
    fn parse(text: &str) -> Option<Self> {
        let number = |text: &str| -> Option<u32> {
            (!text.is_empty() && text.len() <= 2 && text.bytes().all(|byte| byte.is_ascii_digit()))
                .then(|| text.parse().ok())
                .flatten()
        };
        let (hour_text, rest) = text.split_once(':')?;
        let hour = number(hour_text).filter(|hour| *hour <= 23)?;
        let minutes = number(rest.get(..2)?).filter(|minutes| *minutes <= 59)?;
        let meridiem = match rest.get(2..)? {
            "" => Meridiem::Absent,
            written => Meridiem::Attached(meridiem(written).filter(|_| (1..=12).contains(&hour))?),
        };
        Some(Self {
            hour,
            hour_size: hour_text.len(),
            minutes,
            meridiem,
        })
    }
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
    use super::{MERIDIEMS, SEPARATORS, Speakable, WHOLE_TOKENS};

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
            ("service.go:42", "service dot go, line 42"),
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

    /// A citation's lines read after a pause; a clock time reads as a time. Only a path with a
    /// file extension or a folder is a citation (`narration::citation`), so `word:42` keeps its
    /// colon rather than inventing a line.
    #[test]
    fn reads_cited_lines_and_clock_times() {
        for (caption, spoken) in [
            ("tcp.go:88-95", "tcp dot go, lines 88 to 95"),
            ("tcp.go:88–95.", "tcp dot go, lines 88 to 95."),
            ("service.go:42", "service dot go, line 42"),
            ("handler.go:L12", "handler dot go, line 12"),
            ("handler.go:L12-20", "handler dot go, lines 12 to 20"),
            (
                "internal/user/service.go:42,",
                "internal slash user slash service dot go, line 42,",
            ),
            ("(see `service.go:43`).", "(see service dot go, line 43)."),
            (
                "crates/core/Makefile:3",
                "crates slash core slash Makefile, line 3",
            ),
            ("service.go:0", "service dot go:0"),
            ("tcp.go:88-x", "tcp dot go:88-x"),
            ("main:42", "main:42"),
            ("localhost:8080", "localhost:8080"),
            ("http://example.com:80", "http://example dot com:80"),
            ("at 12:30.", "at 12 30."),
            ("9:05", "9 oh 5"),
            ("09:30", "9 30"),
            ("00:15", "0 15"),
            ("9:00", "9 o'clock"),
            ("12:00", "12 o'clock"),
            ("15:00", "15 hundred"),
            ("7:00pm", "7 P M"),
            ("7:30PM,", "7 30 P M,"),
            ("11:05am", "11 oh 5 A M"),
            ("9:00 AM", "9 A M"),
            ("at 7:30 a.m. sharp", "at 7 30 A M. sharp"),
            ("12:00 pm, then", "12 P M, then"),
            ("9:00, am I late", "9 o'clock, am I late"),
            ("I am at 9:00", "I am at 9 o'clock"),
            ("13:00 PM", "13 hundred PM"),
            ("13:30pm", "13:30pm"),
            ("16:9", "16:9"),
            ("3:1", "3:1"),
            ("12:30:45", "12:30:45"),
            ("24:00", "24:00"),
            ("12:60", "12:60"),
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
        for (written, spoken) in MERIDIEMS {
            for caption in [format!("7:30{written}"), format!("7:30 {written}")] {
                assert_eq!(
                    Speakable::from_caption(&caption).text(),
                    format!("7 30 {spoken}"),
                    "{caption}"
                );
            }
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

    /// The rewritten citation and time keep the map: each number maps one to one, a rewritten
    /// colon or dash snaps to where it was written, and both directions stay monotonic.
    #[test]
    fn cited_lines_and_clock_times_keep_the_offset_map() {
        let caption = "See tcp.go:88-95 at 09:05 now";
        let speakable = Speakable::from_caption(caption);
        let text = speakable.text();
        assert_eq!(text, "See tcp dot go, lines 88 to 95 at 9 oh 5 now");
        let at = |word: &str| text.find(word).expect("spoken");
        let colon = caption.find(':').expect("written");
        assert_eq!(speakable.caption_offset(at("lines")), colon);
        assert_eq!(speakable.caption_offset(at("88") + 1), colon + 2);
        assert_eq!(speakable.caption_offset(at("to")), colon + 3);
        assert_eq!(speakable.caption_offset(at("95")), colon + 4);
        assert_eq!(speakable.speakable_offset(colon + 4), at("95"));
        let time = caption.find("09").expect("written");
        assert_eq!(speakable.caption_offset(at("9 oh")), time + 1);
        assert_eq!(speakable.speakable_offset(time), at("9 oh"));
        assert_eq!(speakable.caption_offset(at("oh")), time + 2);
        assert_eq!(speakable.caption_offset(at("5 now")), time + 4);
        assert_eq!(speakable.speakable_offset(caption.len() - 3), at("now"));
        let mut previous = 0;
        for offset in 0..=text.len() {
            let mapped = speakable.caption_offset(offset);
            assert!(
                mapped >= previous && mapped <= caption.len(),
                "monotonic at {offset}"
            );
            previous = mapped;
        }
        let mut previous = 0;
        for offset in (0..=caption.len()).filter(|offset| caption.is_char_boundary(*offset)) {
            let mapped = speakable.speakable_offset(offset);
            assert!(
                mapped >= previous && mapped <= text.len(),
                "monotonic at {offset}"
            );
            previous = mapped;
        }
    }
}
