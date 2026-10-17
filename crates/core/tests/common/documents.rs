//! A shared PDF for document scenarios: the source and its reader's report.

use boardai_core::input::Input;
use boardai_core::sources::{
    DocumentFormat, SourceId, SourceInfo, SourceInput, SourceKind, SourceState,
};

/// The paper's id in these scenarios.
pub const PAPER: SourceId = SourceId(7);

/// A seven-page paper with two figures, each with a diagram, a table and a claim.
pub const DIGEST: &str = "# Digest: Unbundled Transactions
Read: 7 pages; figures by Gemini 3.8 Flash

## Sections
### 2 Architecture (pp. 2–3)
Clients talk to proxies; a sequencer hands out versions.
### 4 Recovery (pp. 5–6)
The transaction system is replaced as a whole.

## Figures
### Figure 1 (p. 3): The architecture of the layered store.
A client first asks the proxies for a read version. The sequencer hands it out.
<diagram title=\"Figure 1\">
<node id=\"proxies\" label=\"Proxies\" kind=\"service\"/>
<node id=\"sequencer\" label=\"Sequencer\" kind=\"service\"/>
<edge from=\"proxies\" to=\"sequencer\" label=\"get read version\"/>
</diagram>
### Figure 2 (p. 4): The commit pipeline.
A commit goes from the proxy to the resolvers, then to the log servers.
<diagram title=\"Figure 2\">
<node id=\"resolver\" label=\"Resolver\" kind=\"service\"/>
<node id=\"tlog\" label=\"Log server\" kind=\"db\"/>
<edge from=\"resolver\" to=\"tlog\" label=\"commit\"/>
</diagram>

## Tables
### Table 1 (p. 7): Latency by operation.
| Operation | p50 (ms) |
|---|---|
| read | 1 |

## Claims
- Recovery replaces the whole transaction system. (p. 5)
";

pub fn paper() -> SourceInfo {
    SourceInfo {
        id: PAPER,
        kind: SourceKind::Document {
            format: DocumentFormat::Pdf,
        },
        title: "layered-store.pdf".into(),
        shown_path: "~/Papers/layered-store.pdf".into(),
        files_estimate: None,
    }
}

/// The reader's report: read, with `digest`.
pub fn read_event(digest: &str) -> Input {
    Input::Source {
        source: SourceInput::DocumentRead {
            source: PAPER,
            state: SourceState::Ready {
                detail: "read".into(),
            },
            digest: Some(digest.to_owned()),
        },
    }
}

/// The session opened with no harness.
pub fn opened() -> Input {
    Input::Source {
        source: SourceInput::Opened {
            harness: None,
            project: None,
        },
    }
}
