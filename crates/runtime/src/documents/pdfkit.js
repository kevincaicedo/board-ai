// BoardAI's PDF reader (D-253): macOS PDFKit through `/usr/bin/osascript -l JavaScript`, so PDF
// parsing needs no crate, no `unsafe`, and runs out of BoardAI's process. The runtime embeds this
// file and bounds the run (time, output, pages); it parses exactly the JSON written here.
//
//   text  <absolute path> <max_pages>   → {"v":1,"page_count":N,"locked":false,"pages":[{"lines":[…]}]}
//                                          each line {"text","size","bold","box":[x,y,width,height]}
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

// A tenth of a point is all the runtime's reading order and heading detection need.
function tenth(value) {
  return Math.round(number(value) * 10) / 10;
}

// The font at a character of the page's text, kept for the run of text PDFKit says it covers: a
// paragraph set in one font costs one lookup, not one per line.
function Fonts(attributed) {
  this.attributed = attributed;
  this.begin = 0;
  this.end = 0;
  this.size = 0;
  this.bold = false;
}

Fonts.prototype.at = function (index) {
  if (index >= this.begin && index < this.end) return this;
  var effective = Ref();
  var font = this.attributed.attributeAtIndexEffectiveRange($.NSFontAttributeName, index, effective);
  var range = effective[0];
  var begin = absent(range) ? index : number(range.location);
  var length = absent(range) ? 0 : number(range.length);
  var covered = begin <= index && index < begin + length;
  this.begin = covered ? begin : index;
  this.end = covered ? begin + length : index + 1;
  this.size = absent(font) ? 0 : tenth(font.pointSize);
  this.bold = absent(font) ? false : BOLD.test(String(font.fontName.js));
  return this;
};

// One line: its text (from `begin` in the page's text), its box on the page in PDF points, and
// the font at its first non-space character. A blank line is left out.
function line(selection, pdfPage, text, begin, fonts) {
  var first = text.search(/\S/);
  if (first < 0) return null;
  var bounds = selection.boundsForPage(pdfPage);
  var font = fonts.at(begin + first);
  return {
    text: text,
    size: font.size,
    bold: font.bold,
    box: [tenth(bounds.origin.x), tenth(bounds.origin.y), tenth(bounds.size.width), tenth(bounds.size.height)],
  };
}

// A run of a line's text, one box per line in it: PDFKit ends a line's text with its break, and
// a run that holds more than one line (a macOS that lays out lines otherwise) is sent a line at
// a time, each with its own box, never as one box over all of them.
function pieces(selection, pdfPage, text, begin, fonts, lines) {
  var parts = text.split(/\r\n|\r|\n/);
  var filled = parts.filter(function (part) {
    return /\S/.test(part);
  });
  if (filled.length <= 1) {
    var whole = line(selection, pdfPage, text, begin, fonts);
    if (whole !== null) lines.push(whole);
    return;
  }
  var breaks = text.match(/\r\n|\r|\n/g) || [];
  var offset = begin;
  for (var at = 0; at < parts.length; at++) {
    if (/\S/.test(parts[at])) {
      var piece = pdfPage.selectionForRange($.NSMakeRange(offset, parts[at].length));
      var read = absent(piece) ? null : line(piece, pdfPage, parts[at], offset, fonts);
      if (read !== null) lines.push(read);
    }
    offset += parts[at].length + (at < breaks.length ? breaks[at].length : 0);
  }
}

// The page's lines as PDFKit lays them out, each with its box. Their order here is PDFKit's,
// which differs between macOS versions (macOS 14 reads a table column by column): the runtime
// orders them by where they sit (D-293). A line made of several runs of the text is sent as one
// box per run, so no run is lost, and a run of several lines as one box per line.
function page(document, index) {
  var pdfPage = document.pageAtIndex(index);
  if (absent(pdfPage)) return { lines: [] };
  var attributed = pdfPage.attributedString;
  if (absent(attributed)) return { lines: [] };
  var whole = attributed.string.js;
  if (typeof whole !== 'string' || whole.length === 0) return { lines: [] };
  var selection = pdfPage.selectionForRange($.NSMakeRange(0, whole.length));
  if (absent(selection)) return { lines: [] };
  var byLine = selection.selectionsByLine;
  var count = absent(byLine) ? 0 : number(byLine.count);
  var fonts = new Fonts(attributed);
  var lines = [];
  for (var at = 0; at < count; at++) {
    var one = byLine.objectAtIndex(at);
    var runs = number(one.numberOfTextRangesOnPage(pdfPage));
    for (var run = 0; run < runs; run++) {
      var range = one.rangeAtIndexOnPage(run, pdfPage);
      var begin = number(range.location);
      var text = whole.substr(begin, number(range.length));
      var part = runs === 1 ? one : pdfPage.selectionForRange(range);
      if (!absent(part)) pieces(part, pdfPage, text, begin, fonts, lines);
    }
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
