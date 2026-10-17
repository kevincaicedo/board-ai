use super::{
    CANCEL_PHRASES, DONE_PHRASES, Heard, LABEL_LEAD_INS, START_PHRASES, UNDO_PHRASES, could_label,
    heard, is_start,
};
use crate::conversation::cues::{BACKCHANNELS, has_interruption_cue};

#[test]
fn every_start_phrase_asks_to_draw_with_fillers_and_only_whole() {
    for phrase in START_PHRASES {
        assert!(is_start(phrase), "{phrase}");
        let noisy = format!("Wait, {}.", phrase.to_uppercase());
        assert!(is_start(&noisy), "{noisy}");
        let more = format!("{phrase} the cache layer for the API");
        assert!(!is_start(&more), "{more}");
    }
    assert!(is_start("Let me draw what I mean."));
    assert!(is_start("Hold on, I’ll draw it."));
    for other in ["", "draw me a diagram", "let me think", "show me the code"] {
        assert!(!is_start(other), "{other:?}");
    }
}

#[test]
fn every_control_is_recognized_whole() {
    for phrase in CANCEL_PHRASES {
        assert_eq!(heard(phrase), Heard::Cancel, "{phrase}");
        assert_eq!(
            heard(&format!("Okay, {phrase}.")),
            Heard::Cancel,
            "{phrase}"
        );
    }
    for phrase in UNDO_PHRASES {
        assert_eq!(heard(phrase), Heard::Undo, "{phrase}");
        assert_eq!(heard(&format!("Oh, {phrase}.")), Heard::Undo, "{phrase}");
    }
    for phrase in DONE_PHRASES {
        assert_eq!(heard(phrase), Heard::Done { before: "" }, "{phrase}");
        assert_eq!(
            heard(&format!("Okay, {}.", phrase.to_uppercase())),
            Heard::Done { before: "" },
            "{phrase}"
        );
    }
}

#[test]
fn a_done_at_the_end_keeps_what_came_before_it() {
    assert_eq!(
        heard("And this one is the cache, and done."),
        Heard::Done {
            before: "And this one is the cache"
        }
    );
    assert_eq!(
        heard("Postgres. That's it."),
        Heard::Done { before: "Postgres" }
    );
}

#[test]
fn a_question_finishes_the_drawing() {
    for question in [
        "Does this make sense?",
        "what do you think",
        "Is the cache in the right place?",
        "So, where should the queue go?",
    ] {
        assert_eq!(heard(question), Heard::Question, "{question}");
    }
}

#[test]
fn labels_come_after_a_lead_in_or_alone() {
    let cases = [
        ("…and this one is the cache", "cache"),
        ("This is the client.", "client"),
        ("That's our Postgres", "Postgres"),
        ("Here's the API gateway.", "API gateway"),
        ("this is called the source of truth", "source of truth"),
        ("Okay, Redis.", "Redis"),
        ("the load balancer", "load balancer"),
        ("“Postgres”", "Postgres"),
    ];
    for (said, label) in cases {
        assert_eq!(heard(said), Heard::Label(label.to_owned()), "{said}");
    }
    for lead_in in LABEL_LEAD_INS {
        let said = format!("and {lead_in} the cache");
        assert_eq!(heard(&said), Heard::Label("cache".to_owned()), "{said}");
    }
}

#[test]
fn longer_words_are_talk_and_backchannels_nothing() {
    for talk in [
        "this is where all the requests from the phone go",
        "the client calls the API and the API reads Postgres",
        "this is just a rough idea",
        "this is",
    ] {
        assert_eq!(heard(talk), Heard::Talk, "{talk}");
    }
    for nothing in BACKCHANNELS.iter().chain(START_PHRASES) {
        assert_eq!(heard(nothing), Heard::Nothing, "{nothing}");
    }
    assert_eq!(heard(""), Heard::Nothing);
}

/// The hint shows while a label is being said, and not for controls or questions.
#[test]
fn a_label_on_its_way_shows_the_hint() {
    for partial in [
        "and this",
        "and this one is the",
        "this is the cache",
        "Redis",
    ] {
        assert!(could_label(partial), "{partial}");
    }
    for partial in [
        "",
        "okay",
        "does this",
        "what do you",
        "this is where all the requests from the phone go",
    ] {
        assert!(!could_label(partial), "{partial:?}");
    }
}

/// A drawing control said over BoardAI's line must stop it: "wait" makes it an interruption,
/// and "let me draw" alone is a short remark (rule e).
#[test]
fn asking_to_draw_over_the_voice_interrupts() {
    assert!(has_interruption_cue("wait, let me draw it"));
    assert!(!BACKCHANNELS.contains(&"let me draw"));
}
