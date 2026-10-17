//! What leaving the settings page (or "Start talking") does (D-103): a first run starts the
//! session, a change restarts it, and so does fixing what kept a running session deaf
//! (review 2, finding 5).

use crate::session::Hearing;
use crate::setup::commands::{Finish, finish_plan};

fn after(started: bool, changed: bool, hearing: Hearing, ready: bool) -> Option<Finish> {
    finish_plan(started, changed, hearing).map(|after| after(ready))
}

#[test]
fn a_deaf_session_restarts_once_the_microphone_and_hearing_pass() {
    assert_eq!(
        after(true, false, Hearing::Deaf, true),
        Some(Finish::Restart),
        "the microphone was allowed on the page: the next session hears"
    );
    assert_eq!(
        after(true, false, Hearing::Deaf, false),
        Some(Finish::Close),
        "still deaf with nothing saved: the page closes on the session as it is"
    );
    assert!(
        finish_plan(true, false, Hearing::Listening).is_none(),
        "nothing to apply"
    );
    assert!(
        finish_plan(true, false, Hearing::NoAudio).is_none(),
        "no audio unit: nothing on the page fixes that"
    );
}

#[test]
fn a_change_restarts_and_a_first_run_starts_once_nothing_blocks() {
    assert_eq!(
        after(true, true, Hearing::Listening, true),
        Some(Finish::Restart)
    );
    assert_eq!(
        after(true, true, Hearing::Listening, false),
        Some(Finish::Refuse),
        "a blocker keeps the page open and says why"
    );
    assert_eq!(
        after(false, false, Hearing::NoAudio, true),
        Some(Finish::Start)
    );
    assert_eq!(
        after(false, true, Hearing::NoAudio, false),
        Some(Finish::Refuse)
    );
}
