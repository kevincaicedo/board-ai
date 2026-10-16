//! Tool declarations for the talker (LLD §3.3): each tool's name, description and JSON Schema,
//! so every provider adapter declares the same set once per session. A test checks that each
//! schema's properties are exactly what `ToolCall::parse` accepts.

use serde_json::{Value, json};

use super::TOOL_NAMES;

#[derive(Clone, Debug, PartialEq)]
pub struct ToolSchema {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

fn object(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn text(description: &str) -> Value {
    json!({ "type": "string", "description": description })
}

fn links() -> Value {
    json!({
        "type": "array",
        "items": { "type": "string" },
        "description": "Related sessions, board elements, projects or file:line references.",
    })
}

/// Every tool, in [`TOOL_NAMES`] order.
#[must_use]
pub fn schemas() -> Vec<ToolSchema> {
    let entries = repo_tools()
        .into_iter()
        .chain(harness_tools())
        .chain(memory_tools())
        .chain(view_tools());
    TOOL_NAMES
        .iter()
        .zip(entries)
        .map(|(name, (description, input_schema))| ToolSchema {
            name,
            description,
            input_schema,
        })
        .collect()
}

/// `read_file`, `search_code`, `list_dir`.
fn repo_tools() -> [(&'static str, Value); 3] {
    let line = json!({ "type": "integer", "minimum": 1 });
    [
        (
            "Read lines of a file in the open repo. Lines are 1-based; the result is bounded.",
            object(
                json!({ "path": text("Repo-relative path."), "start_line": line, "end_line": line }),
                &["path"],
            ),
        ),
        (
            "Search the open repo with a regular expression; respects .gitignore.",
            object(
                json!({ "query": text("Regular expression."), "glob": text("Optional file glob.") }),
                &["query"],
            ),
        ),
        (
            "List a directory of the open repo.",
            object(
                json!({ "path": text("Repo-relative directory.") }),
                &["path"],
            ),
        ),
    ]
}

/// `research`, `review`, `propose_change`.
fn harness_tools() -> [(&'static str, Value); 3] {
    [
        (
            "Start a deep, read-only research task in the coding harness, for a question about \
             the open repo's code that needs many files read. Never for general knowledge you \
             can explain yourself. Returns at once; its output arrives later as untrusted task \
             output.",
            object(
                json!({
                    "question": text("The precise question."),
                    "depth": { "type": "string", "enum": ["quick", "deep"] },
                }),
                &["question", "depth"],
            ),
        ),
        (
            "Start a read-only code review in the coding harness. Returns at once.",
            object(json!({ "scope": text("What to review.") }), &["scope"]),
        ),
        (
            "Propose a code change. Nothing changes yet: when your reply ends, BoardAI reads the \
             instruction and its new branch to the user and asks for a yes itself, and only a \
             spoken yes runs it. One proposal at a time.",
            object(
                json!({ "instruction": text(
                    "One short sentence (at most 200 bytes) saying exactly what will change. It \
                     is read to the user word for word, and exactly it runs."
                ) }),
                &["instruction"],
            ),
        ),
    ]
}

/// `remember`, `add_task`, `recall`, `open_session`.
fn memory_tools() -> [(&'static str, Value); 4] {
    [
        (
            "Remember something: what the user stated (kept), what you inferred (only \
             proposed; ask whether to keep it), or a note they asked for.",
            object(
                json!({
                    "kind": { "type": "string", "enum": ["stated", "inferred", "note"] },
                    "text": text("In the user's words where possible."),
                    "about": {
                        "type": "string",
                        "enum": ["you", "systems"],
                        "description": "Who it is about: the user (preferences, how they \
                                        learn) or their systems.",
                    },
                    "links": links(),
                }),
                &["kind", "text", "about"],
            ),
        ),
        (
            "Add a task the user asked for.",
            object(
                json!({
                    "title": text("Short imperative title."),
                    "project": text("Project name."),
                    "links": links(),
                }),
                &["title"],
            ),
        ),
        (
            "Search past sessions, notes and memories.",
            object(json!({ "query": text("What to look for.") }), &["query"]),
        ),
        (
            "Bring back a past session's board, drawn at once beside its recap.",
            object(
                json!({ "id": text("The session id, or words from its title.") }),
                &["id"],
            ),
        ),
    ]
}

/// `show_library`, `show_tasks`, `show_memory`, `play_session`.
fn view_tools() -> [(&'static str, Value); 4] {
    [
        (
            "Show the user's past sessions as a library of cards.",
            object(json!({}), &[]),
        ),
        (
            "Show the Now list: open tasks for one project, or for all.",
            object(
                json!({ "project": text("Project name; omit for every project.") }),
                &[],
            ),
        ),
        (
            "Show what you remember about the user and their systems, and recent notes.",
            object(json!({}), &[]),
        ),
        (
            "Replay a past session like a film.",
            object(
                json!({ "id": text("The session id, or words from its title.") }),
                &["id"],
            ),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::schemas;
    use crate::tools::{TOOL_NAMES, ToolCall, declared_fields};

    #[test]
    fn every_tool_has_a_schema_in_order() {
        let names: Vec<&str> = schemas().iter().map(|schema| schema.name).collect();
        assert_eq!(names, TOOL_NAMES);
    }

    /// The schemas and the parser agree: same properties, required fields alone parse, and a
    /// missing required field fails.
    #[test]
    fn schema_properties_match_what_the_parser_accepts() {
        for schema in schemas() {
            let properties = schema.input_schema["properties"]
                .as_object()
                .expect("properties");
            let mut declared: Vec<&str> = properties.keys().map(String::as_str).collect();
            let mut accepted = declared_fields(schema.name).to_vec();
            declared.sort_unstable();
            accepted.sort_unstable();
            assert_eq!(declared, accepted, "{}", schema.name);
            let required: Vec<&str> = schema.input_schema["required"]
                .as_array()
                .expect("required")
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect();
            let minimal: serde_json::Map<String, serde_json::Value> = required
                .iter()
                .map(|field| ((*field).to_owned(), sample(properties, field)))
                .collect();
            let input = serde_json::Value::Object(minimal.clone()).to_string();
            assert!(
                ToolCall::parse(schema.name, &input).is_ok(),
                "{} {input}",
                schema.name
            );
            for field in &required {
                let mut missing = minimal.clone();
                missing.remove(*field);
                let input = serde_json::Value::Object(missing).to_string();
                assert!(
                    ToolCall::parse(schema.name, &input).is_err(),
                    "{} without {field}",
                    schema.name
                );
            }
        }
    }

    fn sample(
        properties: &serde_json::Map<String, serde_json::Value>,
        field: &str,
    ) -> serde_json::Value {
        let property = &properties[field];
        if let Some(first) = property["enum"]
            .as_array()
            .and_then(|values| values.first())
        {
            return first.clone();
        }
        match property["type"].as_str() {
            Some("integer") => serde_json::json!(1),
            Some("array") => serde_json::json!([]),
            _ => serde_json::json!("x"),
        }
    }
}
