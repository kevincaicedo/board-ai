//! Typed text as speech in: a line becomes the inputs a finished spoken utterance would give
//! (LLD §2.2). Used by the text dev loop and the app's dev hook while there is no mic.

use super::inbox::{SessionClosed, SpeechInLink};
use crate::limits::TYPED_MS_PER_WORD;

/// Says `text` as one utterance: voice start, a partial, voice end, then the final. The voiced
/// length is estimated per word, so the core's barge-in rules (a cue word, four words, 700 ms
/// of voice) treat it as they would treat speech: "mm-hm" over BoardAI is a backchannel and
/// "wait, why Redis first?" an interruption.
pub async fn say_typed(link: &SpeechInLink, text: &str) -> Result<(), SessionClosed> {
    let text = text.trim();
    let words = u32::try_from(text.split_whitespace().count()).unwrap_or(u32::MAX);
    let voiced_ms = words.max(1).saturating_mul(TYPED_MS_PER_WORD);
    link.voice_start().await?;
    link.transcript(text.to_owned(), false).await?;
    link.voice_end(voiced_ms).await?;
    link.transcript(text.to_owned(), true).await
}

#[cfg(test)]
mod tests {
    use boardai_core::input::Input;

    use super::say_typed;
    use crate::limits::TYPED_MS_PER_WORD;
    use crate::session::SessionChannels;
    use crate::session::inbox::Envelope;

    #[tokio::test]
    async fn a_line_is_one_finished_utterance() {
        let mut channels = SessionChannels::new();
        assert!(
            say_typed(&channels.speech_in(), "  wait, why Redis first? ")
                .await
                .is_ok()
        );
        let mut inputs = Vec::new();
        while let Ok(Envelope::Input(input)) = channels.receiver.try_recv() {
            inputs.push(input);
        }
        let text = "wait, why Redis first?".to_owned();
        assert_eq!(
            inputs,
            vec![
                Input::VoiceStart,
                Input::Transcript {
                    text: text.clone(),
                    is_final: false
                },
                Input::VoiceEnd {
                    voiced_ms: 4 * TYPED_MS_PER_WORD
                },
                Input::Transcript {
                    text,
                    is_final: true
                },
            ]
        );
    }
}
