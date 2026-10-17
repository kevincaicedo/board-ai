//! Line boxes recorded from PDFKit on macOS 27 (`pdfkit.js text` on the fixtures), their tables'
//! cells as macOS 14 reports them (their boxes from the fixture's own geometry, `make.py`),
//! column by column, and the order a two-column page must read in. One box a line, as a table.

use super::Recorded;

/// `figure-table.pdf` page 3, as PDFKit on macOS 27 reports it: each table row one line.
#[rustfmt::skip]
pub const FIGURE_TABLE_PAGE_3: &[Recorded] = &[
    ("3 Evaluation", 72.0, 693.2, 70.7, 12.0, true),
    ("We measured each operation on a three-machine cluster. Reads are served by the storage servers", 72.0, 676.7, 438.5, 10.0, false),
    ("alone, so they are the fastest; commits pay for the resolver check and the log push, as Figure 1 shows.", 72.0, 663.7, 454.7, 10.0, false),
    ("Table 1: Latency by operation.", 72.0, 644.9, 121.1, 9.0, false),
    ("Operation p50 (ms) p99 (ms)", 76.0, 627.9, 317.0, 9.0, true),
    ("Get read version 0.4 1.2", 76.0, 612.9, 292.5, 9.0, false),
    ("Read 0.3 0.9", 76.0, 597.9, 292.5, 9.0, false),
    ("Commit 2.1 6.8", 76.0, 582.9, 292.5, 9.0, false),
    ("Range read (100 keys) 1.1 3.5", 76.0, 567.9, 292.5, 9.0, false),
    ("3", 303.5, 37.9, 5.0, 9.0, false),
];

#[rustfmt::skip]
pub const FIGURE_TABLE_ROWS: &[&str] = &[
    "Operation p50 (ms) p99 (ms)",
    "Get read version 0.4 1.2",
    "Read 0.3 0.9",
    "Commit 2.1 6.8",
    "Range read (100 keys) 1.1 3.5",
];

/// Its table's cells, column by column, as the macOS 14 runner read them.
#[rustfmt::skip]
pub const FIGURE_TABLE_CELLS: &[Recorded] = &[
    ("Operation", 76.0, 627.9, 42.5, 9.0, true),
    ("Get read version", 76.0, 612.9, 66.5, 9.0, false),
    ("Read", 76.0, 597.9, 21.5, 9.0, false),
    ("Commit", 76.0, 582.9, 31.0, 9.0, false),
    ("Range read (100 keys)", 76.0, 567.9, 91.5, 9.0, false),
    ("p50 (ms)", 256.0, 627.9, 37.0, 9.0, true),
    ("0.4", 256.0, 612.9, 12.5, 9.0, false),
    ("0.3", 256.0, 597.9, 12.5, 9.0, false),
    ("2.1", 256.0, 582.9, 12.5, 9.0, false),
    ("1.1", 256.0, 567.9, 12.5, 9.0, false),
    ("p99 (ms)", 356.0, 627.9, 37.0, 9.0, true),
    ("1.2", 356.0, 612.9, 12.5, 9.0, false),
    ("0.9", 356.0, 597.9, 12.5, 9.0, false),
    ("6.8", 356.0, 582.9, 12.5, 9.0, false),
    ("3.5", 356.0, 567.9, 12.5, 9.0, false),
];

/// `columns.pdf` page 1 in macOS 27's order, which reads the right column before the left
/// column's table and its last paragraph.
#[rustfmt::skip]
pub const COLUMNS_PAGE_1: &[Recorded] = &[
    ("A Replicated Log, Set in Two Columns", 141.5, 691.9, 329.1, 18.0, true),
    ("A two-page fixture for the reading order of columns and tables", 155.3, 657.5, 301.4, 11.0, false),
    ("1 Introduction", 72.0, 621.2, 80.0, 12.0, true),
    ("A replicated log keeps the same sequence of", 72.0, 609.7, 199.6, 10.0, false),
    ("records on several machines, so that losing one", 72.0, 597.7, 211.8, 10.0, false),
    ("machine loses no record. Each record is", 72.0, 585.7, 178.4, 10.0, false),
    ("appended by a leader and copied to the followers", 72.0, 573.7, 218.5, 10.0, false),
    ("before it counts as written.", 72.0, 561.7, 116.7, 10.0, false),
    ("This fixture prints that idea in two columns, the", 72.0, 549.7, 205.6, 10.0, false),
    ("way most papers are printed. Both columns share", 72.0, 537.7, 220.1, 10.0, false),
    ("every baseline, so a reader that sorts the page by", 72.0, 525.7, 219.6, 10.0, false),
    ("height alone reads a line of the left column, then a", 72.0, 513.7, 221.2, 10.0, false),
    ("line of the right one, and gives itself away.", 72.0, 501.7, 185.7, 10.0, false),
    ("2 Roles", 72.0, 477.2, 42.7, 12.0, true),
    ("Table 1 lists the roles in a group of five replicas.", 72.0, 465.7, 211.2, 10.0, false),
    ("Only the leader accepts appends; the followers", 72.0, 453.7, 207.9, 10.0, false),
    ("copy and acknowledge them, and a learner", 72.0, 441.7, 191.2, 10.0, false),
    ("copies without a vote.", 72.0, 429.7, 96.2, 10.0, false),
    ("Table 1: Replica roles in a group of five.", 72.0, 417.9, 158.6, 9.0, false),
    ("arrives.", 318.0, 633.7, 32.8, 10.0, false),
    ("Followers learn of new records in batches. The", 318.0, 621.7, 207.3, 10.0, false),
    ("leader sends each follower the records after the", 318.0, 609.7, 211.2, 10.0, false),
    ("last one it acknowledged, and a follower that has", 318.0, 597.7, 215.7, 10.0, false),
    ("fallen too far behind is sent a snapshot instead of", 318.0, 585.7, 217.4, 10.0, false),
    ("the records it missed.", 318.0, 573.7, 95.0, 10.0, false),
    ("3 Recovery", 318.0, 549.2, 64.0, 12.0, true),
    ("When the leader stops answering, each follower", 318.0, 537.7, 212.9, 10.0, false),
    ("waits out a random timeout and then asks the", 318.0, 525.7, 201.8, 10.0, false),
    ("others for their votes. The first to win a majority", 318.0, 513.7, 207.9, 10.0, false),
    ("becomes the new leader and goes on from the", 318.0, 501.7, 206.2, 10.0, false),
    ("last record a majority holds.", 318.0, 489.7, 122.3, 10.0, false),
    ("A record that only the old leader held is dropped.", 318.0, 477.7, 215.7, 10.0, false),
    ("That is safe, because no client was ever told that", 318.0, 465.7, 216.2, 10.0, false),
    ("the record had been written.", 318.0, 453.7, 124.5, 10.0, false),
    ("Role Count Votes", 76.0, 405.9, 185.5, 9.0, true),
    ("Leader 1 yes", 76.0, 393.9, 175.0, 9.0, false),
    ("Follower 3 yes", 76.0, 381.9, 175.0, 9.0, false),
    ("Learner 1 no", 76.0, 369.9, 171.0, 9.0, false),
    ("A write is durable once a majority of the voting", 72.0, 345.7, 204.5, 10.0, false),
    ("replicas holds it. With four voters, three", 72.0, 333.7, 172.8, 10.0, false),
    ("acknowledgements are enough, so one slow", 72.0, 321.7, 197.3, 10.0, false),
    ("follower never delays a commit, and the leader", 72.0, 309.7, 206.8, 10.0, false),
    ("can answer the client as soon as the third one", 72.0, 297.7, 203.4, 10.0, false),
    ("1", 303.5, 37.9, 5.0, 9.0, false),
];

#[rustfmt::skip]
pub const COLUMNS_PAGE_1_ROWS: &[&str] = &[
    "Role Count Votes",
    "Leader 1 yes",
    "Follower 3 yes",
    "Learner 1 no",
];

#[rustfmt::skip]
pub const COLUMNS_PAGE_1_CELLS: &[Recorded] = &[
    ("Role", 76.0, 405.9, 19.5, 9.0, true),
    ("Leader", 76.0, 393.9, 28.0, 9.0, false),
    ("Follower", 76.0, 381.9, 34.0, 9.0, false),
    ("Learner", 76.0, 369.9, 31.0, 9.0, false),
    ("Count", 176.0, 405.9, 26.0, 9.0, true),
    ("1", 176.0, 393.9, 5.0, 9.0, false),
    ("3", 176.0, 381.9, 5.0, 9.0, false),
    ("1", 176.0, 369.9, 5.0, 9.0, false),
    ("Votes", 237.0, 405.9, 24.5, 9.0, true),
    ("yes", 237.0, 393.9, 14.0, 9.0, false),
    ("yes", 237.0, 381.9, 14.0, 9.0, false),
    ("no", 237.0, 369.9, 10.0, 9.0, false),
];

/// `columns.pdf` page 2: a table across both columns, above them. macOS 27 joins each of its
/// rows into a line across the gutter.
#[rustfmt::skip]
pub const COLUMNS_PAGE_2: &[Recorded] = &[
    ("Table 2: Write latency and throughput by group size.", 72.0, 705.9, 208.6, 9.0, false),
    ("Replicas p50 write (ms) p99 write (ms) Writes/s", 76.0, 693.9, 385.0, 9.0, true),
    ("3 1.8 4.9 61,000", 76.0, 681.9, 377.5, 9.0, false),
    ("5 2.3 6.1 54,000", 76.0, 669.9, 377.5, 9.0, false),
    ("7 2.9 7.7 47,000", 76.0, 657.9, 377.5, 9.0, false),
    ("4 Evaluation", 72.0, 621.2, 70.7, 12.0, true),
    ("Table 2 shows that each pair of replicas added to", 72.0, 609.7, 217.9, 10.0, false),
    ("a group costs about half a millisecond at the", 72.0, 597.7, 195.1, 10.0, false),
    ("median. The extra replicas buy tolerance of more", 72.0, 585.7, 217.3, 10.0, false),
    ("failures, not speed.", 72.0, 573.7, 84.5, 10.0, false),
    ("Throughput falls more slowly than latency rises,", 72.0, 561.7, 210.6, 10.0, false),
    ("because the leader batches the appends that", 72.0, 549.7, 200.1, 10.0, false),
    ("arrive while it waits for acknowledgements, and a", 72.0, 537.7, 217.3, 10.0, false),
    ("larger group waits a little longer and so sends", 318.0, 633.7, 201.2, 10.0, false),
    ("larger batches.", 318.0, 621.7, 66.1, 10.0, false),
    ("5 Lessons", 318.0, 597.2, 58.7, 12.0, true),
    ("Keep the voting group small and add learners for", 318.0, 585.7, 216.2, 10.0, false),
    ("reads. Measure the slowest follower that a", 318.0, 573.7, 187.3, 10.0, false),
    ("majority must wait for, not the average one, since", 318.0, 561.7, 217.9, 10.0, false),
    ("that follower sets the latency of every write.", 318.0, 549.7, 191.2, 10.0, false),
    ("2", 303.5, 37.9, 5.0, 9.0, false),
];

#[rustfmt::skip]
pub const COLUMNS_PAGE_2_ROWS: &[&str] = &[
    "Replicas p50 write (ms) p99 write (ms) Writes/s",
    "3 1.8 4.9 61,000",
    "5 2.3 6.1 54,000",
    "7 2.9 7.7 47,000",
];

/// Its cells on macOS 14: none of them crosses the gutter, so only their rows' shape says the
/// table is full width.
#[rustfmt::skip]
pub const COLUMNS_PAGE_2_CELLS: &[Recorded] = &[
    ("Replicas", 76.0, 693.9, 37.0, 9.0, true),
    ("3", 76.0, 681.9, 5.0, 9.0, false),
    ("5", 76.0, 669.9, 5.0, 9.0, false),
    ("7", 76.0, 657.9, 5.0, 9.0, false),
    ("p50 write (ms)", 226.0, 693.9, 60.5, 9.0, true),
    ("1.8", 226.0, 681.9, 12.5, 9.0, false),
    ("2.3", 226.0, 669.9, 12.5, 9.0, false),
    ("2.9", 226.0, 657.9, 12.5, 9.0, false),
    ("p99 write (ms)", 326.0, 693.9, 60.5, 9.0, true),
    ("4.9", 326.0, 681.9, 12.5, 9.0, false),
    ("6.1", 326.0, 669.9, 12.5, 9.0, false),
    ("7.7", 326.0, 657.9, 12.5, 9.0, false),
    ("Writes/s", 426.0, 693.9, 35.0, 9.0, true),
    ("61,000", 426.0, 681.9, 27.5, 9.0, false),
    ("54,000", 426.0, 669.9, 27.5, 9.0, false),
    ("47,000", 426.0, 657.9, 27.5, 9.0, false),
];

/// The left column whole (its table's rows in place), then the right one, then the page number
/// from the gutter.
#[rustfmt::skip]
pub const COLUMNS_PAGE_1_READ: &[&str] = &[
    "A Replicated Log, Set in Two Columns",
    "A two-page fixture for the reading order of columns and tables",
    "1 Introduction",
    "A replicated log keeps the same sequence of",
    "records on several machines, so that losing one",
    "machine loses no record. Each record is",
    "appended by a leader and copied to the followers",
    "before it counts as written.",
    "This fixture prints that idea in two columns, the",
    "way most papers are printed. Both columns share",
    "every baseline, so a reader that sorts the page by",
    "height alone reads a line of the left column, then a",
    "line of the right one, and gives itself away.",
    "2 Roles",
    "Table 1 lists the roles in a group of five replicas.",
    "Only the leader accepts appends; the followers",
    "copy and acknowledge them, and a learner",
    "copies without a vote.",
    "Table 1: Replica roles in a group of five.",
    "Role Count Votes",
    "Leader 1 yes",
    "Follower 3 yes",
    "Learner 1 no",
    "A write is durable once a majority of the voting",
    "replicas holds it. With four voters, three",
    "acknowledgements are enough, so one slow",
    "follower never delays a commit, and the leader",
    "can answer the client as soon as the third one",
    "arrives.",
    "Followers learn of new records in batches. The",
    "leader sends each follower the records after the",
    "last one it acknowledged, and a follower that has",
    "fallen too far behind is sent a snapshot instead of",
    "the records it missed.",
    "3 Recovery",
    "When the leader stops answering, each follower",
    "waits out a random timeout and then asks the",
    "others for their votes. The first to win a majority",
    "becomes the new leader and goes on from the",
    "last record a majority holds.",
    "A record that only the old leader held is dropped.",
    "That is safe, because no client was ever told that",
    "the record had been written.",
    "1",
];
