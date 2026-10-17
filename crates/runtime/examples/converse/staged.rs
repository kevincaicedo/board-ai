//! Staged diagrams in an answer (T-035): the elements each op draws, the parts each reveal
//! draws by kind, and the parts the talker wrote in its blocks, so a map's breadth is measured.

use boardai_core::protocol::{DrawOp, StagedPart};

/// The parts of an answer's staged diagrams, by kind.
#[derive(Clone, Copy, Debug, Default)]
pub struct StagedCounts {
    pub diagrams: u32,
    pub sections: u32,
    pub details: u32,
    pub containers: u32,
    pub boxes: u32,
    pub records: u32,
    pub fields: u32,
    pub notes: u32,
    pub edges: u32,
}

impl StagedCounts {
    pub fn count(&mut self, part: &StagedPart) {
        match part {
            StagedPart::Frame { .. } => self.containers += 1,
            StagedPart::Node { .. } | StagedPart::Cells { .. } => self.boxes += 1,
            StagedPart::Record { fields, .. } => {
                self.records += 1;
                self.fields += u32::try_from(fields.len()).unwrap_or(u32::MAX);
            }
            StagedPart::Note { .. } => self.notes += 1,
            StagedPart::Edge { .. } => self.edges += 1,
            StagedPart::Legend { .. } => {}
        }
    }

    /// The answer's staged parts on one line, after the parts its blocks held.
    pub fn line(&self, written: u32) -> String {
        format!(
            "{written} parts written · {} diagrams ({} details), {} sections · {} containers, {} \
             boxes, {} records ({} fields), {} notes, {} edges",
            self.diagrams,
            self.details,
            self.sections,
            self.containers,
            self.boxes,
            self.records,
            self.fields,
            self.notes,
            self.edges
        )
    }
}

/// The parts the talker wrote inside its `<diagram>` and `<detail>` blocks, staged or not.
pub fn written_parts(text: &str) -> u32 {
    let mut parts = 0;
    let mut rest = text;
    while let Some(open) = ["<diagram", "<detail"]
        .iter()
        .filter_map(|tag| rest.find(tag))
        .min()
    {
        let block = &rest[open..];
        let end = ["</diagram>", "</detail>", "<reveal"]
            .iter()
            .filter_map(|tag| block.find(tag))
            .min()
            .unwrap_or(block.len());
        let body = &block[..end];
        parts += [
            "<node", "<record", "<edge", "<note", "<cells", "<pill", "<struct", "<state",
        ]
        .iter()
        .map(|tag| u32::try_from(body.matches(tag).count()).unwrap_or(u32::MAX))
        .sum::<u32>();
        rest = &block[end.max(1)..];
    }
    parts
}

/// The elements an op draws: a reveal's parts with their fields and cells, a row's cells and
/// the row, one for anything else.
pub fn elements_of(op: &DrawOp) -> u32 {
    let count = |items: usize| u32::try_from(items).unwrap_or(u32::MAX);
    match op {
        DrawOp::Reveal { parts, .. } => parts
            .iter()
            .map(|part| match part {
                StagedPart::Record { fields, .. } => count(fields.len()) + 1,
                StagedPart::Cells { cells, .. } => count(cells.len()) + 1,
                StagedPart::Frame { .. }
                | StagedPart::Node { .. }
                | StagedPart::Note { .. }
                | StagedPart::Edge { .. }
                | StagedPart::Legend { .. } => 1,
            })
            .sum(),
        DrawOp::Cells { cells, .. } => count(cells.len()) + 1,
        DrawOp::Node { .. }
        | DrawOp::Edge { .. }
        | DrawOp::Annotate { .. }
        | DrawOp::Strike { .. }
        | DrawOp::Trace { .. }
        | DrawOp::Participant { .. }
        | DrawOp::Message { .. }
        | DrawOp::Write { .. } => 1,
    }
}
