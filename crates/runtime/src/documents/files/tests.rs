use super::{Attachment, FileModel, display_name, parse_answer, request_body, takes_files};
use crate::llm::Effort;
use crate::llm::ProviderError;
use crate::settings::Secret;

fn gemini() -> FileModel {
    FileModel {
        name: "Gemini 3.8 Flash".into(),
        base_url: "https://openrouter.ai/api/v1".into(),
        key: Secret::new("sk-or-test".into()),
        model: "google/gemini-3.8-flash".into(),
        route: vec!["google-ai-studio".into()],
        effort: Effort::Low,
    }
}

/// The request as OpenRouter documents file input: a `file` part with a data URL, the parser
/// plugin held to `native` (no third party's OCR, law 10), the role's route, no stream.
#[test]
fn a_page_range_goes_as_a_native_file_part() {
    let body = request_body(
        &gemini(),
        "Read pages 1 to 2.",
        &Attachment::Pdf {
            file_name: "paper-pages-1-2.pdf",
            base64: "JVBERi0=",
        },
    );
    assert_eq!(body["model"], "google/gemini-3.8-flash");
    assert_eq!(body["stream"], false);
    assert_eq!(body["messages"][0]["role"], "system");
    let parts = &body["messages"][1]["content"];
    assert_eq!(parts[0]["text"], "Read pages 1 to 2.");
    assert_eq!(parts[1]["type"], "file");
    assert_eq!(parts[1]["file"]["filename"], "paper-pages-1-2.pdf");
    assert_eq!(
        parts[1]["file"]["file_data"],
        "data:application/pdf;base64,JVBERi0="
    );
    assert_eq!(body["plugins"][0]["id"], "file-parser");
    assert_eq!(body["plugins"][0]["pdf"]["engine"], "native");
    assert_eq!(body["provider"]["order"][0], "google-ai-studio");
    assert_eq!(body["provider"]["allow_fallbacks"], false);
    assert_eq!(body["reasoning"]["effort"], "low");
    assert!(
        !body.to_string().contains("sk-or-test"),
        "the key goes only in a header"
    );
    let images = [("image/png".to_owned(), "iVBO".to_owned())];
    let body = request_body(&gemini(), "Describe.", &Attachment::Images(&images));
    assert_eq!(
        body["messages"][1]["content"][1]["image_url"]["url"],
        "data:image/png;base64,iVBO"
    );
}

#[test]
fn the_listing_says_who_takes_files() {
    let fixture = |name: &str| {
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/openrouter")
                .join(name),
        )
        .unwrap_or_default()
    };
    assert!(takes_files(&fixture("listing_gemini-3.8-flash.json")));
    assert!(takes_files(&fixture("listing_claude-haiku-5.5.json")));
    assert!(!takes_files(&fixture("listing_gpt-oss-120b.json")));
    assert!(!takes_files(b"not json"));
    assert!(!takes_files(br#"{"data":{"architecture":{}}}"#));
}

#[test]
fn answers_parse_with_their_usage_and_failures_keep_their_category() {
    let answer = parse_answer(
        br##"{"choices":[{"message":{"role":"assistant","content":"# Digest: P\n"},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":3100,"completion_tokens":800,"cost":0.0019}}"##,
    );
    let Ok(answer) = answer else {
        panic!("an answer: {answer:?}");
    };
    assert_eq!(answer.markdown, "# Digest: P\n");
    assert!(!answer.cut);
    assert_eq!(
        (answer.prompt_tokens, answer.completion_tokens),
        (3100, 800)
    );
    assert_eq!(answer.cost_usd, Some(0.0019));
    let cut = parse_answer(
        br##"{"choices":[{"message":{"content":"# Digest: P\n## Sec"},"finish_reason":"length"}]}"##,
    );
    assert!(cut.is_ok_and(|answer| answer.cut));
    assert!(matches!(
        parse_answer(br#"{"error":{"message":"No endpoints found that support file input"}}"#),
        Err(ProviderError::Provider { .. })
    ));
    assert!(matches!(
        parse_answer(br#"{"choices":[{"message":{"content":"  "}}]}"#),
        Err(ProviderError::BadOutput { .. })
    ));
    assert!(matches!(
        parse_answer(b"<html>"),
        Err(ProviderError::BadOutput { .. })
    ));
}

#[test]
fn models_are_named_as_people_say_them() {
    assert_eq!(display_name("google/gemini-3.8-flash"), "Gemini 3.8 Flash");
    assert_eq!(
        display_name("anthropic/claude-haiku-5.5"),
        "Claude Haiku 5.5"
    );
    assert_eq!(display_name("openai/gpt-6-luna"), "GPT-6 Luna");
    assert_eq!(display_name("qwen/qwen3.8-27b:free"), "Qwen3.8 27b");
}
