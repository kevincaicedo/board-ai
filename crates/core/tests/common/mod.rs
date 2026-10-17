//! Helpers shared by the simulator scenario tests.
#![allow(dead_code, reason = "each test file uses a different subset")]

pub mod documents;

use boardai_core::effect::{Effect, LogRecord};
use boardai_core::history::{AssistantPart, Message, UserPart};
use boardai_core::protocol::{OrbState, UiEvent};
use boardai_core::sim::Sim;
use boardai_core::time::SessionTime;
use boardai_core::{CoreConfig, MicMode, RepoReads, RequestStart};

/// The demo from PRODUCT §5, as the talker would narrate it.
pub const DEMO: &str = concat!(
    r#"<plan steps="Client|Edge|LB|API"/>Sure. It starts in the "#,
    r#"<node id="client" label="Browser" kind="actor"/>browser, then <step n="2"/>hits the "#,
    r#"<node id="cdn" label="CDN" near="client"/><edge from="client" to="cdn"/>CDN at the edge. "#,
    r#"Then the <step n="3"/><node id="lb" label="Load balancer" near="cdn"/>"#,
    r#"<edge from="cdn" to="lb"/>load balancer spreads it across "#,
    r#"<node id="api" label="API pod" near="lb" stack="3"/><edge from="lb" to="api"/>three API pods."#,
);

pub fn config() -> CoreConfig {
    CoreConfig {
        talker_name: "Claude".to_owned(),
        mic: MicMode::OpenMic,
        session_context: None,
        suggestion: None,
        repo_reads: RepoReads::NotOffered,
        request_start: RequestStart::AtTurnEnd,
    }
}

pub fn assert_clean(sim: &Sim) {
    assert!(
        sim.violations().is_empty(),
        "invariants: {:#?}",
        sim.violations()
    );
}

pub fn count(sim: &Sim, matches: impl Fn(&Effect) -> bool) -> usize {
    sim.effects().filter(|(_, effect)| matches(effect)).count()
}

pub fn times(sim: &Sim, matches: impl Fn(&Effect) -> bool) -> Vec<SessionTime> {
    sim.effects()
        .filter(|(_, effect)| matches(effect))
        .map(|(at, _)| at)
        .collect()
}

pub fn op_starts(sim: &Sim) -> Vec<(SessionTime, &UiEvent)> {
    sim.ui()
        .filter(|(_, event)| matches!(event, UiEvent::OpStart { .. }))
        .collect()
}

pub fn orbs(sim: &Sim) -> Vec<OrbState> {
    sim.ui()
        .filter_map(|(_, event)| {
            if let UiEvent::Orb { state, .. } = event {
                Some(*state)
            } else {
                None
            }
        })
        .collect()
}

pub fn dropped(sim: &Sim) -> usize {
    count(sim, |effect| {
        matches!(effect, Effect::Log(LogRecord::Dropped { .. }))
    })
}

/// The text of every message, for readable assertions.
pub fn transcript(sim: &Sim) -> Vec<String> {
    sim.core()
        .conversation()
        .messages()
        .iter()
        .map(|message| match message {
            Message::User { parts } => {
                let text: Vec<String> = parts
                    .iter()
                    .map(|part| match part {
                        UserPart::Text { text } => text.clone(),
                        UserPart::ToolResult { content, .. } => format!("<result>{content}"),
                    })
                    .collect();
                format!("user: {}", text.join(" | "))
            }
            Message::Assistant { parts, raw } => {
                let text: Vec<String> = parts
                    .iter()
                    .map(|part| match part {
                        AssistantPart::Text { text } => text.clone(),
                        AssistantPart::ToolUse { name, .. } => format!("<tool_use {name}>"),
                    })
                    .collect();
                let stored = if raw.is_some() { "stored" } else { "heard" };
                format!("assistant({stored}): {}", text.join(" | "))
            }
            Message::System { text } => format!("system: {text}"),
        })
        .collect()
}

/// Everything the talker was sent in request `index` but its own turns, as one text: system
/// notes, the user's turns and the tool results.
pub fn request_text(sim: &Sim, index: usize) -> String {
    sim.talker.requests[index]
        .messages
        .iter()
        .flat_map(|message| match message {
            Message::User { parts } => parts
                .iter()
                .map(|part| match part {
                    UserPart::Text { text } => text.clone(),
                    UserPart::ToolResult { content, .. } => content.clone(),
                })
                .collect(),
            Message::System { text } => vec![text.clone()],
            Message::Assistant { .. } => Vec::new(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The system messages of request `index`, as one text.
pub fn system_texts(sim: &Sim, index: usize) -> String {
    sim.talker.requests[index]
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::System { text } => Some(text.clone()),
            Message::User { .. } | Message::Assistant { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n---\n")
}

/// Request `index` with what the task-output fences hold taken out: BoardAI's own words and the
/// user's, where a source's words must never be.
pub fn outside_fences(sim: &Sim, index: usize) -> String {
    request_text(sim, index)
        .split("<task-output>")
        .enumerate()
        .map(|(at, chunk)| {
            if at == 0 {
                chunk.to_owned()
            } else {
                chunk
                    .split_once("</task-output>")
                    .map_or(String::new(), |(_, after)| after.to_owned())
            }
        })
        .collect()
}
