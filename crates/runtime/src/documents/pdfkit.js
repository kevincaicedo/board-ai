// BoardAI's PDF reader (D-253): macOS PDFKit through `/usr/bin/osascript -l JavaScript`, so PDF
// parsing needs no crate, no `unsafe`, and runs out of BoardAI's process. The runtime embeds this
// file and bounds the run (time, output, pages); it parses exactly the JSON written here.
//
//   text  <absolute path> <max_pages>   → {"v":1,"page_count":N,"locked":false,"pages":[{"lines":[…]}]}
//   pages <absolute path> <first> <last> → the same shape for pages [first, last] (1-based) alone,
//                                          with "first": first; pages past the end are left out
//   split <absolute path> <first> <last> → the base64 of a PDF of pages [first, last] (1-based)
//
// Failures are JSON on stdout too ({"v":1,"error":"…"}), never an exception: osascript would put
// an exception on stderr, which PDFKit also fills with its own logging. No network, no writes.

ObjC.import('Foundation');
ObjC.import('PDFKit');
ObjC.import('AppKit');

// Font names that read as bold for heading detection.
var BOLD = /Bold|Black|Heavy|Semibold|SemiBold|Demi/;

function fail(reason) {
  return JSON.stringify({ v: 1, error: reason });
}

// JXA hands some numbers back as strings ("14" for pageCount): every number goes through here.
function number(value) {
  var parsed = Number(value);
  return isFinite(parsed) ? parsed : 0;
}

function absent(object) {
  return object === null || object === undefined || (object.isNil && object.isNil());
}

function open(path) {
  if (typeof path !== 'string' || path.charAt(0) !== '/') return null;
  var document = $.PDFDocument.alloc.initWithURL($.NSURL.fileURLWithPath(path));
  return absent(document) ? null : document;
}

// One line: its text, and the font at its first non-space character. The attributed string's
// text is the page's text, so line offsets index it directly.
function line(attributed, text, begin) {
  var first = text.search(/\S/);
  if (first < 0) return { text: '', size: 0, bold: false };
  var font = attributed.attributeAtIndexEffectiveRange($.NSFontAttributeName, begin + first, null);
  if (absent(font)) return { text: text, size: 0, bold: false };
  return {
    text: text,
    size: Math.round(number(font.pointSize) * 10) / 10,
    bold: BOLD.test(String(font.fontName.js)),
  };
}

function page(document, index) {
  var pdfPage = document.pageAtIndex(index);
  if (absent(pdfPage)) return { lines: [] };
  var attributed = pdfPage.attributedString;
  if (absent(attributed)) return { lines: [] };
  var whole = attributed.string.js;
  if (typeof whole !== 'string' || whole.length === 0) return { lines: [] };
  var lines = [];
  var begin = 0;
  // \r\n counts as one break; a lone \r or \n each ends a line. Empty lines are kept: they are
  // the paragraph breaks.
  var parts = whole.split(/\r\n|\r|\n/);
  var breaks = whole.match(/\r\n|\r|\n/g) || [];
  for (var at = 0; at < parts.length; at++) {
    lines.push(line(attributed, parts[at], begin));
    begin += parts[at].length + (at < breaks.length ? breaks[at].length : 0);
  }
  return { lines: lines };
}

function text(path, maxPages) {
  var document = open(path);
  if (document === null) return fail('unreadable');
  var count = number(document.pageCount);
  if (document.isLocked) return JSON.stringify({ v: 1, page_count: count, locked: true, pages: [] });
  var pages = [];
  var limit = Math.min(count, Math.max(0, Math.floor(number(maxPages))));
  for (var index = 0; index < limit; index++) pages.push(page(document, index));
  return JSON.stringify({ v: 1, page_count: count, locked: false, pages: pages });
}

// A page read (D-256) needs only its pages: opening the document is quick, extracting every page
// of a long one is not (600 pages take ~13 s), so a read never waits on the whole text layer.
function pages(path, first, last) {
  var document = open(path);
  if (document === null) return fail('unreadable');
  var count = number(document.pageCount);
  if (document.isLocked) return JSON.stringify({ v: 1, page_count: count, locked: true, pages: [] });
  var from = Math.floor(number(first));
  var to = Math.floor(number(last));
  if (from < 1 || to < from) return fail('bad range');
  var read = [];
  for (var index = from - 1; index < Math.min(to, count); index++) read.push(page(document, index));
  return JSON.stringify({ v: 1, page_count: count, locked: false, first: from, pages: read });
}

function split(path, first, last) {
  var document = open(path);
  if (document === null) return fail('unreadable');
  if (document.isLocked) return fail('locked');
  var count = number(document.pageCount);
  var from = Math.floor(number(first));
  var to = Math.floor(number(last));
  if (from < 1 || to < from || to > count) return fail('bad range');
  var part = $.PDFDocument.alloc.init;
  for (var index = from - 1; index < to; index++) {
    var original = document.pageAtIndex(index);
    if (absent(original)) return fail('unreadable');
    // A copy: inserting the page itself would move it out of the source document.
    part.insertPageAtIndex(original.copy, part.pageCount);
  }
  var data = part.dataRepresentation;
  if (absent(data)) return fail('unreadable');
  return data.base64EncodedStringWithOptions(0).js;
}

function run(argv) {
  var mode = argv[0];
  if (mode === 'text' && argv.length === 3) return text(argv[1], argv[2]);
  if (mode === 'pages' && argv.length === 4) return pages(argv[1], argv[2], argv[3]);
  if (mode === 'split' && argv.length === 4) return split(argv[1], argv[2], argv[3]);
  return fail('usage');
}
