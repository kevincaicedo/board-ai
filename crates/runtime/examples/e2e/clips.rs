//! The user's lines, spoken by macOS `say` into AIFF files and decoded to the mic ring's 16 kHz,
//! with where the speech in each clip begins and ends (the reference for every latency).

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use boardai_runtime::audio::{MIC_RATE_HZ, read_audio_file};
use boardai_runtime::session::{Cue, Script};

/// A 10 ms window louder than this (RMS) is speech, for measuring a clip's speech span.
const SPEECH_RMS: f32 = 0.02;
const WINDOW_FRAMES: usize = MIC_RATE_HZ as usize / 100;

pub struct Clip {
    pub text: String,
    pub cue: Cue,
    pub samples: Arc<[f32]>,
    /// Milliseconds into the clip where speech begins and ends.
    pub speech_begin_ms: u64,
    pub speech_end_ms: u64,
}

impl Clip {
    /// The clip asks to interrupt: it is timed into BoardAI's answer.
    pub fn is_barge_in(&self) -> bool {
        matches!(self.cue, Cue::IntoSpeech { .. })
    }
}

/// Speaks every line of `script` with `say` (its default voice, or `voice`) into `folder`.
pub fn speak(script: &Script, folder: &Path, voice: Option<&str>) -> Result<Vec<Clip>, String> {
    std::fs::create_dir_all(folder).map_err(|error| format!("{}: {error}", folder.display()))?;
    let mut clips = Vec::with_capacity(script.lines.len());
    for (index, line) in script.lines.iter().enumerate() {
        let path = folder.join(format!("line-{}.aiff", index + 1));
        let mut say = Command::new("say");
        if let Some(voice) = voice {
            say.args(["-v", voice]);
        }
        let status = say
            .arg("-o")
            .arg(&path)
            .arg(&line.text)
            .status()
            .map_err(|error| format!("say: {error}"))?;
        if !status.success() {
            return Err(format!("say failed on line {}", index + 1));
        }
        let samples = read_audio_file(&path).map_err(|error| error.to_string())?;
        let (speech_begin_ms, speech_end_ms) =
            speech_span(&samples).ok_or_else(|| format!("line {} is silent", index + 1))?;
        clips.push(Clip {
            text: line.text.clone(),
            cue: line.cue,
            samples: samples.into(),
            speech_begin_ms,
            speech_end_ms,
        });
    }
    Ok(clips)
}

/// The first and last 10 ms window with speech, in milliseconds.
fn speech_span(samples: &[f32]) -> Option<(u64, u64)> {
    let loud: Vec<usize> = samples
        .chunks(WINDOW_FRAMES)
        .enumerate()
        .filter(|(_, window)| {
            let energy = window.iter().map(|sample| sample * sample).sum::<f32>();
            (energy / window.len() as f32).sqrt() > SPEECH_RMS
        })
        .map(|(index, _)| index)
        .collect();
    let first = *loud.first()?;
    let last = *loud.last()?;
    Some((first as u64 * 10, (last as u64 + 1) * 10))
}
