//! The reading order on line boxes recorded from PDFKit on macOS 27 (`pdfkit.js text` on the
//! fixtures), and the same pages as macOS 14 reports them: each table row split into its cells
//! (their boxes from the fixture's own geometry, `make.py`), column by column, the order the
//! macOS 14 CI runner gave `figure-table.pdf`'s table. Both must read the same.

mod recorded;

use recorded::{
    COLUMNS_PAGE_1, COLUMNS_PAGE_1_CELLS, COLUMNS_PAGE_1_READ, COLUMNS_PAGE_1_ROWS, COLUMNS_PAGE_2,
    COLUMNS_PAGE_2_CELLS, COLUMNS_PAGE_2_ROWS, FIGURE_TABLE_CELLS, FIGURE_TABLE_PAGE_3,
    FIGURE_TABLE_ROWS,
};

use super::{Bounds, LineBox, reading_order};
use crate::documents::pdfkit::PdfLine;
use crate::limits::PDF_PAGE_BOXES_MAX;

/// `(text, x, y, width, size, bold)`: a box as PDFKit reports it; its height is its size.
type Recorded = (&'static str, f64, f64, f64, f32, bool);

fn boxes(recorded: &[Recorded]) -> Vec<LineBox> {
    recorded
        .iter()
        .filter_map(|(text, x, y, width, size, bold)| {
            LineBox::new(
                PdfLine {
                    text: (*text).to_owned(),
                    size: *size,
                    bold: *bold,
                },
                Bounds {
                    x: *x,
                    y: *y,
                    width: *width,
                    height: f64::from(*size),
                },
            )
        })
        .collect()
}

fn texts(lines: &[PdfLine]) -> Vec<&str> {
    lines.iter().map(|line| line.text.as_str()).collect()
}

/// The page as macOS 14 reports it: `cells` (column by column) where macOS 27 has the rows
/// `rows`, then the rest in macOS 27's order.
fn as_on_macos_14(recorded: &[Recorded], rows: &[&str], cells: &[Recorded]) -> Vec<Recorded> {
    let mut page = cells.to_vec();
    page.extend(recorded.iter().filter(|line| !rows.contains(&line.0)));
    page
}

#[test]
fn a_table_reads_row_by_row_on_macos_14_as_on_macos_27() {
    let on_27 = reading_order(boxes(FIGURE_TABLE_PAGE_3));
    let on_14 = reading_order(boxes(&as_on_macos_14(
        FIGURE_TABLE_PAGE_3,
        FIGURE_TABLE_ROWS,
        FIGURE_TABLE_CELLS,
    )));
    assert_eq!(on_14, on_27);
    let expected: Vec<&str> = FIGURE_TABLE_PAGE_3.iter().map(|line| line.0).collect();
    assert_eq!(texts(&on_27), expected);
    assert!(
        on_14[4].bold,
        "a row is in its first cell's font: {:?}",
        on_14[4]
    );
    assert!((on_14[4].size - 9.0).abs() < f32::EPSILON);
}

#[test]
fn two_columns_read_a_column_at_a_time_with_a_table_in_one() {
    let on_27 = reading_order(boxes(COLUMNS_PAGE_1));
    let on_14 = reading_order(boxes(&as_on_macos_14(
        COLUMNS_PAGE_1,
        COLUMNS_PAGE_1_ROWS,
        COLUMNS_PAGE_1_CELLS,
    )));
    assert_eq!(texts(&on_27), COLUMNS_PAGE_1_READ);
    assert_eq!(on_14, on_27);
}

#[test]
fn a_page_number_in_the_gutter_comes_after_both_columns() {
    // A little left of the gutter's middle: by its centre alone it would end the left column.
    let page: Vec<Recorded> = COLUMNS_PAGE_1
        .iter()
        .map(|line| match line.0 {
            "1" => ("1", 300.0, line.2, line.3, line.4, line.5),
            _ => *line,
        })
        .collect();
    assert_eq!(texts(&reading_order(boxes(&page))), COLUMNS_PAGE_1_READ);
}

#[test]
fn a_table_across_both_columns_reads_across_them_above_the_columns() {
    let expected = [
        "Table 2: Write latency and throughput by group size.",
        "Replicas p50 write (ms) p99 write (ms) Writes/s",
        "3 1.8 4.9 61,000",
        "5 2.3 6.1 54,000",
        "7 2.9 7.7 47,000",
        "4 Evaluation",
        "Table 2 shows that each pair of replicas added to",
        "a group costs about half a millisecond at the",
        "median. The extra replicas buy tolerance of more",
        "failures, not speed.",
        "Throughput falls more slowly than latency rises,",
        "because the leader batches the appends that",
        "arrive while it waits for acknowledgements, and a",
        "larger group waits a little longer and so sends",
        "larger batches.",
        "5 Lessons",
        "Keep the voting group small and add learners for",
        "reads. Measure the slowest follower that a",
        "majority must wait for, not the average one, since",
        "that follower sets the latency of every write.",
        "2",
    ];
    let on_27 = reading_order(boxes(COLUMNS_PAGE_2));
    let on_14 = reading_order(boxes(&as_on_macos_14(
        COLUMNS_PAGE_2,
        COLUMNS_PAGE_2_ROWS,
        COLUMNS_PAGE_2_CELLS,
    )));
    assert_eq!(texts(&on_27), expected);
    assert_eq!(on_14, on_27);
}

/// What the order must never be: rows by height across the whole page. The two-column pages
/// share their baselines, so it joins a left line and a right line into one, which the tests
/// above would catch.
fn by_height_alone(recorded: &[Recorded]) -> Vec<String> {
    let mut lines = recorded.to_vec();
    lines.sort_by(|a, b| b.2.total_cmp(&a.2).then(a.1.total_cmp(&b.1)));
    let mut rows: Vec<(f64, String)> = Vec::new();
    for (text, _, y, ..) in lines {
        match rows.last_mut() {
            Some((anchor, row)) if *anchor - y <= 2.0 => {
                row.push(' ');
                row.push_str(text);
            }
            Some(_) | None => rows.push((y, text.to_owned())),
        }
    }
    rows.into_iter().map(|(_, row)| row).collect()
}

#[test]
fn the_column_pages_would_catch_a_reader_that_sorts_by_height() {
    for page in [COLUMNS_PAGE_1, COLUMNS_PAGE_2] {
        let read: Vec<String> = reading_order(boxes(page))
            .into_iter()
            .map(|line| line.text)
            .collect();
        let sorted = by_height_alone(page);
        assert_ne!(read, sorted);
        assert!(
            sorted.len() < read.len(),
            "lines of both columns share a baseline and are joined: {sorted:#?}"
        );
    }
}

#[test]
fn the_order_never_depends_on_the_order_pdfkit_gives() {
    for page in [FIGURE_TABLE_PAGE_3, COLUMNS_PAGE_1, COLUMNS_PAGE_2] {
        let read = reading_order(boxes(page));
        let mut reversed = page.to_vec();
        reversed.reverse();
        assert_eq!(reading_order(boxes(&reversed)), read);
        for shift in [1, page.len() / 2, page.len() - 1] {
            let mut rotated = page.to_vec();
            rotated.rotate_left(shift);
            assert_eq!(reading_order(boxes(&rotated)), read);
        }
    }
}

#[test]
fn three_columns_read_left_to_right() {
    let columns = [
        [
            "Left one: the first column's first line of prose",
            "Left two: the first column's second line here",
        ],
        [
            "Middle one: the second column starts here too",
            "Middle two: and it goes on for a second line",
        ],
        [
            "Right one: the third column starts on the same",
            "Right two: baseline as both of the others do",
        ],
    ];
    let mut page: Vec<Recorded> = Vec::new();
    for (x, lines) in [40.0, 220.0, 400.0].into_iter().zip(&columns) {
        for (y, text) in [700.0, 688.0].into_iter().zip(lines) {
            page.push((text, x, y, 160.0, 8.0, false));
        }
    }
    page.reverse();
    let expected: Vec<&str> = columns.iter().flatten().copied().collect();
    assert_eq!(texts(&reading_order(boxes(&page))), expected);
}

/// Body text across the page at 72 to 540, five lines 12 points apart from `top`.
fn body(top: f64) -> Vec<Recorded> {
    [
        "One column of body text runs the whole width of the page here,",
        "and every line of it is long enough to be prose of its own, so",
        "nothing beside it can be a second column of the page's text, and",
        "the lines go on down the page past whatever sits in the margin",
        "until the paragraph ends at the bottom of the block of text.",
    ]
    .into_iter()
    .zip([0.0, 12.0, 24.0, 36.0, 48.0])
    .map(|(text, down)| (text, 72.0, top - down, 468.0, 10.0, false))
    .collect()
}

#[test]
fn a_table_with_long_cells_still_reads_by_rows() {
    // macOS 14's cells of a table whose first column is prose-long, under body text: a long cell
    // beside a short one inside the text is a table, never a gutter.
    let mut page = body(700.0);
    let cells: &[Recorded] = &[
        (
            "Reads served by the storage servers alone",
            76.0,
            600.0,
            180.0,
            9.0,
            false,
        ),
        (
            "Commits that pay for the resolver check",
            76.0,
            588.0,
            170.0,
            9.0,
            false,
        ),
        ("0.3", 356.0, 600.0, 12.5, 9.0, false),
        ("2.1", 356.0, 588.0, 12.5, 9.0, false),
    ];
    page.extend(cells);
    let read = reading_order(boxes(&page));
    assert_eq!(
        texts(&read)[5..],
        [
            "Reads served by the storage servers alone 0.3",
            "Commits that pay for the resolver check 2.1",
        ]
    );
}

#[test]
fn a_note_in_the_margin_is_never_joined_into_the_body() {
    let mut page = body(700.0);
    let note: &[Recorded] = &[
        ("A note in", 12.0, 688.0, 40.0, 8.0, false),
        ("the margin,", 12.0, 676.0, 44.0, 8.0, false),
        ("beside it.", 12.0, 664.0, 38.0, 8.0, false),
    ];
    page.extend(note);
    let read = reading_order(boxes(&page));
    let mut expected: Vec<&str> = note.iter().map(|line| line.0).collect();
    expected.extend(body(700.0).iter().map(|line| line.0));
    assert_eq!(texts(&read), expected);
}

#[test]
fn a_page_whose_lines_run_up_it_keeps_pdfkits_order() {
    // A table turned on its side: each line's box is tall and narrow.
    let page: &[Recorded] = &[
        (
            "Second line, set up the page",
            120.0,
            100.0,
            9.0,
            9.0,
            false,
        ),
        ("First line, set up the page", 100.0, 100.0, 9.0, 9.0, false),
    ];
    let mut tall = boxes(page);
    for line_box in &mut tall {
        line_box.top = line_box.bottom + 150.0;
    }
    let read = reading_order(tall);
    assert_eq!(
        texts(&read),
        [
            "Second line, set up the page",
            "First line, set up the page"
        ]
    );
}

/// `count` lines up the page, the reverse of reading order, 12 points apart.
fn up_the_page(count: usize) -> Vec<LineBox> {
    (0..count)
        .filter_map(|index| {
            LineBox::new(
                PdfLine {
                    text: format!("line {index} of the page"),
                    size: 9.0,
                    bold: false,
                },
                Bounds {
                    x: 72.0,
                    y: 12.0 * f64::from(u32::try_from(index).unwrap_or(u32::MAX)),
                    width: 80.0,
                    height: 9.0,
                },
            )
        })
        .collect()
}

#[test]
fn a_page_past_its_box_bound_keeps_pdfkits_order() {
    let at_the_bound = reading_order(up_the_page(PDF_PAGE_BOXES_MAX));
    assert_eq!(
        at_the_bound.first().map(|line| line.text.as_str()),
        Some(format!("line {} of the page", PDF_PAGE_BOXES_MAX - 1).as_str()),
        "at the bound the page is read top to bottom"
    );
    let past = up_the_page(PDF_PAGE_BOXES_MAX + 1);
    let given: Vec<String> = past
        .iter()
        .map(|line_box| line_box.line.text.clone())
        .collect();
    let read: Vec<String> = reading_order(past)
        .into_iter()
        .map(|line| line.text)
        .collect();
    assert_eq!(read, given, "past it, PDFKit's order, and nothing lost");
}

#[test]
fn a_box_is_parsed_once_into_a_finite_line() {
    let line = |text: &str| PdfLine {
        text: text.to_owned(),
        size: 9.0,
        bold: false,
    };
    let at = |x: f64, y: f64, width: f64, height: f64| Bounds {
        x,
        y,
        width,
        height,
    };
    assert_eq!(LineBox::new(line(" \n "), at(0.0, 0.0, 1.0, 1.0)), None);
    let Some(odd) = LineBox::new(
        line("  two\nlines \n"),
        at(f64::NAN, f64::INFINITY, -4.0, 1e300),
    ) else {
        panic!("a line with text");
    };
    assert_eq!(odd.line.text, "two lines");
    assert!(
        [odd.left, odd.right, odd.bottom, odd.top]
            .iter()
            .all(|value| value.is_finite())
    );
    assert!(odd.right >= odd.left && odd.top >= odd.bottom);
    let Some(far) = LineBox::new(line("far"), at(1e9, -1e9, 10.0, 10.0)) else {
        panic!("a line with text");
    };
    assert_eq!((far.left, far.right), (100_000.0, 100_000.0));
    assert_eq!((far.bottom, far.top), (-100_000.0, -100_000.0));
}
