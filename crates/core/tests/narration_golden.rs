//! Narration parser tests (LLD §2.3, §7): golden files, the split-invariance property at every
//! split point of every golden file, and random input that must never panic or break the bounds.
//!
//! `UPDATE_GOLDEN=1 cargo test -p boardai-core --test narration_golden` rewrites the expected
//! files; review the diff before keeping it.
#![allow(clippy::expect_used, reason = "a broken fixture should fail loudly")]

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use boardai_core::limits::{
    CITATION_TOKEN_MAX_BYTES, CLAUSE_MAX_BYTES, OPS_PER_RESPONSE_MAX, SPOKEN_CALL_MAX_BYTES,
    TOOL_CALLS_PER_RESPONSE_MAX,
};
use boardai_core::narration::spoken_calls::SpokenCall;
use boardai_core::narration::{ClauseOrigin, NarrationOutput, NarrationStream, StreamAnchor};

fn golden_inputs() -> Vec<PathBuf> {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let mut inputs: Vec<PathBuf> = fs::read_dir(&directory)
        .expect("golden directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "narration")
        })
        .collect();
    inputs.sort();
    // The property must have something to run on: an empty directory is a failure, not a pass.
    assert!(inputs.len() >= 5, "golden inputs missing: {inputs:?}");
    inputs
}

/// What a stream gave: its outputs, then the tool calls it kept out of speech (D-179) and the
/// repo reads its speech announced (D-232).
#[derive(Debug, PartialEq, Eq)]
struct Run {
    outputs: Vec<NarrationOutput>,
    spoken_calls: Vec<SpokenCall>,
    announced: Vec<&'static str>,
}

/// Feeds `chunks` in order, then finishes.
fn run(chunks: &[&str]) -> Run {
    let mut stream = NarrationStream::new(0);
    let mut out = Vec::new();
    for chunk in chunks {
        stream.push(chunk, &mut out);
    }
    stream.finish(&mut out);
    Run {
        outputs: out,
        spoken_calls: stream.spoken_calls().to_vec(),
        announced: stream.announced_reads().to_vec(),
    }
}

fn render(run: &Run) -> String {
    let mut text = String::new();
    for output in &run.outputs {
        let _ = match output {
            NarrationOutput::Clause(clause) => {
                let origin = match clause.origin {
                    ClauseOrigin::Talker { caption_begin } => format!("talker@{caption_begin}"),
                    ClauseOrigin::Core => "core".to_owned(),
                };
                writeln!(
                    text,
                    "clause {} [{origin}]\n  caption: {:?}\n  speech:  {:?}",
                    clause.index,
                    clause.caption,
                    clause.speakable.text()
                )
            }
            NarrationOutput::Op(op) => {
                let anchor = match op.anchor {
                    StreamAnchor::Spoken {
                        clause,
                        caption_offset,
                    } => format!("clause {clause} byte {caption_offset}"),
                    StreamAnchor::Unanchored => "unanchored".to_owned(),
                };
                writeln!(text, "op {} at {anchor}: {:?}", op.index, op.op)
            }
            NarrationOutput::Dropped(what) => writeln!(text, "dropped {what:?}"),
        };
    }
    for call in &run.spoken_calls {
        let _ = writeln!(text, "spoken call {}: {:?}", call.tool, call.written);
    }
    if !run.announced.is_empty() {
        let _ = writeln!(text, "announced {}", run.announced.join(", "));
    }
    text
}

#[test]
fn golden_files_match() {
    let update = std::env::var("UPDATE_GOLDEN").is_ok();
    for input_path in golden_inputs() {
        let input = fs::read_to_string(&input_path).expect("readable input");
        let rendered = render(&run(&[&input]));
        let expected_path = input_path.with_extension("expected");
        if update {
            fs::write(&expected_path, &rendered).expect("writable expected file");
            continue;
        }
        let expected = fs::read_to_string(&expected_path).expect("expected file exists");
        assert_eq!(rendered, expected, "{}", input_path.display());
    }
}

/// Feeding any split of the input gives the same outputs as feeding it whole.
#[test]
fn every_golden_file_is_split_invariant_at_every_split_point() {
    for input_path in golden_inputs() {
        let input = fs::read_to_string(&input_path).expect("readable input");
        let whole = run(&[&input]);
        let boundaries: Vec<usize> = (0..=input.len())
            .filter(|index| input.is_char_boundary(*index))
            .collect();
        for &split in &boundaries {
            let halves = run(&[&input[..split], &input[split..]]);
            assert_eq!(halves, whole, "{} split at {split}", input_path.display());
        }
        let chars: Vec<String> = input.chars().map(String::from).collect();
        let one_by_one: Vec<&str> = chars.iter().map(String::as_str).collect();
        assert_eq!(
            run(&one_by_one),
            whole,
            "{} char by char",
            input_path.display()
        );
    }
}

/// xorshift64*, in test code only.
struct Rng(u64);

impl Rng {
    fn below(&mut self, bound: usize) -> usize {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let value = self.0.wrapping_mul(0x2545_f491_4f6c_dd1d);
        usize::try_from(value % u64::try_from(bound.max(1)).unwrap_or(1)).unwrap_or(0)
    }
}

const PIECES: &[&str] = &[
    "<",
    ">",
    "/>",
    "</",
    "\"",
    "'",
    "=",
    " ",
    "\n",
    ".",
    ",",
    "?",
    "node",
    "edge",
    "id",
    "label",
    "x",
    "é",
    "…",
    "<node id=\"a\"",
    "<edge from=\"a\" to=\"b\"/>",
    "<detour/>",
    "<resume/>",
    "word ",
    "getUserByID",
    "L42",
    "&amp;",
    "e.g.",
    "\u{1F600}",
    "->",
    "→",
    "*",
    "_",
    "- ",
    "<annotate line=\"4\" code=\"a->b\"/>",
    "<recap bullets=\"a|b\"/>",
    // Tool calls written as words (D-179), whole and in parts.
    "list_dir ",
    "path ",
    "read_file(",
    ")",
    "`",
    "<page/>",
    // gemma4's forms (D-233) and announced reads (D-232), whole and in parts.
    "tool_code",
    "tool_code:",
    "end_tool_code",
    "[tool_code]",
    "[/tool_code]",
    "print(",
    "list_dir{path:<|\"|>.<|\"|>}",
    "<|\"|>",
    "<channel|>",
    "<|channel>",
    "<|",
    "|>",
    "I'll ",
    "let me ",
    "look at the ",
    "listing ",
    "files",
    "I'll list the files. ",
    "Let me search for ",
    // Staged diagrams (D-187): blocks, their parts, reveals and details, whole and in parts.
    "<diagram id=\"d\" title=\"t\">",
    "<diagram>",
    "</diagram>",
    "<detail of=\"a\">",
    "</detail>",
    "<detail of=\"a\"/>",
    "<reveal id=\"a\"/>",
    "<reveal id=\"d\" deep=\"true\"/>",
    "<record id=\"r\" fields=\"x|y\"/>",
    "<note text=\"n\"/>",
    "<node id=\"g\" kind=\"group\"/>",
];

/// Random input never panics, keeps every clause within its cap, keeps the op cap, and stays
/// split-invariant.
#[test]
fn random_input_never_panics_and_stays_bounded() {
    let mut rng = Rng(0x5eed_cafe_f00d_d00d);
    for _ in 0..300 {
        let mut input = String::new();
        for _ in 0..rng.below(400) {
            input.push_str(PIECES[rng.below(PIECES.len())]);
        }
        let whole = run(&[&input]);
        let mut ops = 0;
        for output in &whole.outputs {
            match output {
                NarrationOutput::Clause(clause) => {
                    assert!(clause.caption.len() <= CLAUSE_MAX_BYTES, "{input:?}");
                }
                NarrationOutput::Op(_) => ops += 1,
                NarrationOutput::Dropped(_) => {}
            }
        }
        assert!(ops <= OPS_PER_RESPONSE_MAX);
        assert!(whole.spoken_calls.len() <= TOOL_CALLS_PER_RESPONSE_MAX);
        // A held name and its whitespace, then one word past the bound.
        let written_max = SPOKEN_CALL_MAX_BYTES + 2 * CITATION_TOKEN_MAX_BYTES;
        assert!(
            whole
                .spoken_calls
                .iter()
                .all(|call| call.written.len() <= written_max),
            "{input:?}"
        );
        let mut announced = whole.announced.clone();
        announced.dedup();
        assert!(announced.len() == whole.announced.len() && announced.len() <= 3);
        let mut cut = rng.below(input.len() + 1);
        while !input.is_char_boundary(cut) {
            cut -= 1;
        }
        assert_eq!(
            run(&[&input[..cut], &input[cut..]]),
            whole,
            "{input:?} at {cut}"
        );
    }
}
