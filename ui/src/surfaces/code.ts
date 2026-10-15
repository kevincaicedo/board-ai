// A light tokenizer for the code card (frame 5): keywords in medium weight, comments in ink-3,
// numbers in ink-2, like the design. It is deliberately not a syntax highlighter: the card is
// for following along, and color stays reserved for meaning.

export type CodeTokenKind = "plain" | "keyword" | "comment" | "number";

export interface CodeToken {
  readonly kind: CodeTokenKind;
  readonly text: string;
}

const KEYWORDS = new Set([
  "func",
  "fn",
  "def",
  "function",
  "if",
  "else",
  "for",
  "while",
  "loop",
  "return",
  "match",
  "switch",
  "case",
  "break",
  "continue",
  "go",
  "defer",
  "select",
  "range",
  "var",
  "let",
  "const",
  "type",
  "struct",
  "enum",
  "interface",
  "impl",
  "trait",
  "class",
  "pub",
  "async",
  "await",
  "import",
  "package",
  "use",
  "from",
  "export",
  "new",
  "try",
  "catch",
  "throw",
]);

const PATTERN =
  /(\/\/.*$|#\s.*$)|(\b\d+(?:\.\d+)?\b)|([A-Za-z_][A-Za-z0-9_]*)|(\s+|[^\sA-Za-z0-9_/#]+|[/#])/g;

export function tokenizeLine(line: string): CodeToken[] {
  const tokens: CodeToken[] = [];
  const push = (kind: CodeTokenKind, text: string): void => {
    const last = tokens[tokens.length - 1];
    if (last !== undefined && last.kind === kind && kind === "plain") {
      tokens[tokens.length - 1] = { kind, text: last.text + text };
    } else {
      tokens.push({ kind, text });
    }
  };
  for (const match of line.matchAll(PATTERN)) {
    const [whole, comment, number, word] = match;
    if (comment !== undefined) {
      push("comment", comment);
    } else if (number !== undefined) {
      push("number", number);
    } else if (word !== undefined) {
      push(KEYWORDS.has(word) ? "keyword" : "plain", word);
    } else {
      push("plain", whole);
    }
  }
  return tokens;
}

/** The path chip's range, like the design: `L42–52`. */
export function lineRange(firstLine: number, count: number): string {
  if (count <= 1) {
    return `L${firstLine}`;
  }
  return `L${firstLine}–${firstLine + count - 1}`;
}
