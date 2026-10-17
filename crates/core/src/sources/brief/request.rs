//! The brief request (D-185): the task message that asks a coding harness to read a code source
//! and answer in the brief's shape. Its bounds sit under the parser's, so an answer that follows
//! them is never cut.

use std::fmt::Write as _;

use super::BriefTopic;
use crate::sources::SourceKind;

/// The task message for a brief about `title` (the source's name) on `topic`. `question` is
/// what the talker or the user asked, when there was a question.
#[must_use]
pub fn request_prompt(
    title: &str,
    kind: SourceKind,
    topic: &BriefTopic,
    question: Option<&str>,
) -> String {
    let scope = match kind {
        SourceKind::CodeFile => format!(
            "the file \"{title}\" in this folder. Read only that file and what it directly needs"
        ),
        SourceKind::CodeFolder | SourceKind::Document { .. } => {
            format!("the code in this folder (\"{title}\")")
        }
    };
    let focus = match topic {
        BriefTopic::Overview => {
            "an overview: what it is for, its main parts and how they connect, and its main flows"
                .to_owned()
        }
        BriefTopic::Component { name } => format!(
            "\"{name}\" in more detail: its internals, its own parts and how they work together"
        ),
    };
    let mut prompt = format!(
        "Read {scope}, and write a research brief for BoardAI, a voice tutor that explains code \
         while drawing it. The brief is about {focus}.\n"
    );
    if let Some(question) = question
        .map(str::trim)
        .filter(|question| !question.is_empty())
    {
        let _ = writeln!(prompt, "The question was: \"{question}\"");
    }
    prompt.push_str(SHAPE);
    prompt
}

/// The answer's shape, with bounds under the parser's own.
const SHAPE: &str = "\
Answer with only this Markdown, in this order, and nothing before or after it:

# Brief: <topic, a few words>
## Overview
<at most 5 sentences, plain enough to be spoken>
## Components
- <id> · <name> · <kind> · <path> — <role, one sentence>
## Relations
- <id> → <id>: <label, a few words>
## Flows
### <flow name>
1. <step, one sentence> (<path>:<line>)
## Diagram
<diagram title=\"<title>\">
<node id=\"<id>\" label=\"<name>\" kind=\"service\"/>
<edge from=\"<id>\" to=\"<id>\" label=\"<label>\"/>
</diagram>
## Details
### <component id>
<at most 4 sentences about its internals; optionally a nested <diagram>…</diagram>>
## Citations
- <path>:<line> or <path>:<first>-<last>

Rules:
- At most 24 components, 32 relations, 4 flows of at most 8 steps, 40 diagram tags, details for \
at most 8 components, and 24 citations.
- An id is lowercase letters, digits and - (\"arena\", \"slot-pool\"); use the same ids in \
Relations, the Diagram and Details. The kind is a word such as crate, module, type, trait, \
function or service.
- The diagram uses only <node id label kind/> (kind: service, db, queue, actor, group, note), \
<edge from to label/>, and in=\"<group id>\" on a node to nest it.
- Paths are relative to this folder. Cite only lines you actually read, and cite every claim.
- Do not modify any file.
";

#[cfg(test)]
mod tests {
    use super::request_prompt;
    use crate::limits::{
        BRIEF_CITATIONS_MAX, BRIEF_COMPONENTS_MAX, BRIEF_DETAILS_MAX, BRIEF_FLOW_STEPS_MAX,
        BRIEF_FLOWS_MAX, BRIEF_RELATIONS_MAX,
    };
    use crate::sources::{BriefTopic, SourceKind};

    #[test]
    fn the_request_names_the_topic_and_the_shape() {
        let prompt = request_prompt(
            "inf-alloc",
            SourceKind::CodeFolder,
            &BriefTopic::component("arena"),
            Some("How does it align?"),
        );
        for expected in [
            "(\"inf-alloc\")",
            "\"arena\" in more detail",
            "How does it align?",
            "# Brief:",
            "## Citations",
            "Do not modify any file.",
        ] {
            assert!(prompt.contains(expected), "{expected:?} in {prompt}");
        }
    }

    /// The prompt's own bounds stay under the parser's, so a compliant answer is never cut.
    #[test]
    fn the_asked_bounds_fit_the_parser() {
        let (components, relations, flows, steps, details, citations) = (24, 32, 4, 8, 8, 24);
        let prompt = request_prompt("x", SourceKind::CodeFolder, &BriefTopic::Overview, None);
        assert!(prompt.contains(&format!(
            "At most {components} components, {relations} relations, {flows} flows of at most \
             {steps} steps"
        )));
        assert!(prompt.contains(&format!("at most {details} components, and {citations}")));
        assert!(components <= BRIEF_COMPONENTS_MAX && relations <= BRIEF_RELATIONS_MAX);
        assert!(flows <= BRIEF_FLOWS_MAX && steps <= BRIEF_FLOW_STEPS_MAX);
        assert!(details <= BRIEF_DETAILS_MAX && citations <= BRIEF_CITATIONS_MAX);
    }
}
