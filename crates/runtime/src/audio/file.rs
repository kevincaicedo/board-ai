//! Audio files as mic input (LLD §7): WAV and AIFF/AIFC PCM decoded to the mic ring's 16 kHz
//! mono, for the end-to-end test and any run without a microphone. macOS `say -o` writes AIFF.
//!
//! Integer PCM (8 to 32 bits) and 32-bit float, any channel count (averaged to mono) and any
//! rate the resampler takes. Compressed formats are refused. Chunk walking is a bounded loop:
//! every step consumes at least a chunk header.

use std::fmt;
use std::path::Path;

use super::MIC_RATE_HZ;
use super::resample::Resampler;
use crate::limits::{AUDIO_FILE_MAX_BYTES, AUDIO_FILE_MAX_MS};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioFileError {
    Io {
        detail: String,
    },
    TooLarge {
        size_bytes: u64,
    },
    TooLong {
        duration_ms: u64,
    },
    /// Not WAV or AIFF, a compressed encoding, or a damaged header.
    Format {
        detail: String,
    },
}

impl fmt::Display for AudioFileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { detail } => write!(formatter, "audio file: {detail}"),
            Self::TooLarge { size_bytes } => write!(
                formatter,
                "audio file is {size_bytes} bytes; the limit is {AUDIO_FILE_MAX_BYTES}"
            ),
            Self::TooLong { duration_ms } => write!(
                formatter,
                "audio file lasts {duration_ms} ms; the limit is {AUDIO_FILE_MAX_MS}"
            ),
            Self::Format { detail } => write!(formatter, "audio file format: {detail}"),
        }
    }
}

impl std::error::Error for AudioFileError {}

/// Reads and decodes `path` to 16 kHz mono.
pub fn read_audio_file(path: &Path) -> Result<Vec<f32>, AudioFileError> {
    let io = |error: std::io::Error| AudioFileError::Io {
        detail: error.to_string(),
    };
    let size_bytes = std::fs::metadata(path).map_err(io)?.len();
    if size_bytes > AUDIO_FILE_MAX_BYTES {
        return Err(AudioFileError::TooLarge { size_bytes });
    }
    decode_audio(&std::fs::read(path).map_err(io)?)
}

/// Decodes a WAV or AIFF file's bytes to 16 kHz mono.
pub fn decode_audio(bytes: &[u8]) -> Result<Vec<f32>, AudioFileError> {
    let pcm = match bytes.get(..4) {
        Some(b"RIFF") => wav(bytes)?,
        Some(b"FORM") => aiff(bytes)?,
        _ => return Err(format("neither WAV nor AIFF")),
    };
    pcm.to_mic_rate()
}

fn format(detail: &str) -> AudioFileError {
    AudioFileError::Format {
        detail: detail.to_owned(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Encoding {
    Integer { bits: u16 },
    Float32,
}

/// Interleaved samples as found in the file.
struct Pcm<'a> {
    rate_hz: u32,
    channels: u16,
    encoding: Encoding,
    big_endian: bool,
    data: &'a [u8],
}

impl Pcm<'_> {
    fn bytes_per_sample(&self) -> usize {
        match self.encoding {
            Encoding::Integer { bits } => usize::from(bits.div_ceil(8)),
            Encoding::Float32 => 4,
        }
    }

    fn sample(&self, raw: &[u8]) -> f32 {
        let mut word = [0_u8; 4];
        let width = raw.len();
        // Left-aligned in a 32-bit word, so every integer width scales the same way.
        for (index, byte) in raw.iter().enumerate() {
            let position = if self.big_endian {
                index
            } else {
                width - 1 - index
            };
            word[position] = *byte;
        }
        let value = u32::from_be_bytes(word);
        match self.encoding {
            Encoding::Float32 => f32::from_bits(value),
            Encoding::Integer { bits: 8 } if !self.big_endian => {
                // WAV's 8-bit PCM is unsigned.
                f32::from(word[0]) / 128.0 - 1.0
            }
            Encoding::Integer { .. } => i32::from_be_bytes(word) as f32 / 2_147_483_648.0,
        }
    }

    fn to_mic_rate(&self) -> Result<Vec<f32>, AudioFileError> {
        let width = self.bytes_per_sample();
        let channels = usize::from(self.channels.max(1));
        let frame_bytes = width * channels;
        let frames = self.data.len() / frame_bytes;
        let duration_ms = frames as u64 * 1_000 / u64::from(self.rate_hz.max(1));
        if duration_ms > AUDIO_FILE_MAX_MS {
            return Err(AudioFileError::TooLong { duration_ms });
        }
        let mono = self.data.chunks_exact(frame_bytes).map(|frame| {
            let sum: f32 = frame.chunks_exact(width).map(|raw| self.sample(raw)).sum();
            sum / channels as f32
        });
        if self.rate_hz == MIC_RATE_HZ {
            return Ok(mono.collect());
        }
        let mut resampler = Resampler::new(self.rate_hz, MIC_RATE_HZ)
            .map_err(|_| format(&format!("{} Hz cannot be converted", self.rate_hz)))?;
        let mut out = Vec::with_capacity(frames * MIC_RATE_HZ as usize / self.rate_hz as usize + 1);
        let input: Vec<f32> = mono.collect();
        resampler.push(&input, |sample| out.push(sample));
        // The filter's tail: the last input frames come out only after its delay.
        let tail = vec![0.0; resampler.delay_input_frames() as usize];
        resampler.push(&tail, |sample| out.push(sample));
        Ok(out)
    }
}

/// `(id, body)` of each chunk after a 12-byte container header, in order.
fn chunks(bytes: &[u8], big_endian: bool) -> Vec<([u8; 4], &[u8])> {
    let mut found = Vec::new();
    let mut offset = 12;
    while offset + 8 <= bytes.len() {
        let mut id = [0_u8; 4];
        id.copy_from_slice(&bytes[offset..offset + 4]);
        let mut size = [0_u8; 4];
        size.copy_from_slice(&bytes[offset + 4..offset + 8]);
        let size = if big_endian {
            u32::from_be_bytes(size)
        } else {
            u32::from_le_bytes(size)
        } as usize;
        let begin = offset + 8;
        let end = begin.saturating_add(size).min(bytes.len());
        found.push((id, &bytes[begin..end]));
        // Chunks are padded to an even size.
        offset = begin.saturating_add(size).saturating_add(size % 2);
    }
    found
}

fn u16_at(bytes: &[u8], at: usize, big_endian: bool) -> Option<u16> {
    let pair: [u8; 2] = bytes.get(at..at + 2)?.try_into().ok()?;
    Some(if big_endian {
        u16::from_be_bytes(pair)
    } else {
        u16::from_le_bytes(pair)
    })
}

fn u32_at(bytes: &[u8], at: usize, big_endian: bool) -> Option<u32> {
    let quad: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
    Some(if big_endian {
        u32::from_be_bytes(quad)
    } else {
        u32::from_le_bytes(quad)
    })
}

const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xfffe;

fn wav(bytes: &[u8]) -> Result<Pcm<'_>, AudioFileError> {
    if bytes.get(8..12) != Some(b"WAVE") {
        return Err(format("a RIFF file that is not WAVE"));
    }
    let found = chunks(bytes, false);
    let fmt = found
        .iter()
        .find(|(id, _)| id == b"fmt ")
        .map(|(_, body)| *body)
        .ok_or_else(|| format("WAV without a fmt chunk"))?;
    let data = found
        .iter()
        .find(|(id, _)| id == b"data")
        .map(|(_, body)| *body)
        .ok_or_else(|| format("WAV without a data chunk"))?;
    let field = |at| u16_at(fmt, at, false).ok_or_else(|| format("short fmt chunk"));
    let mut tag = field(0)?;
    let channels = field(2)?;
    let rate_hz = u32_at(fmt, 4, false).ok_or_else(|| format("short fmt chunk"))?;
    let bits = field(14)?;
    if tag == WAVE_FORMAT_EXTENSIBLE {
        // The sub-format GUID starts with the real format tag.
        tag = field(24)?;
    }
    let encoding = match (tag, bits) {
        (WAVE_FORMAT_PCM, 8..=32) => Encoding::Integer { bits },
        (WAVE_FORMAT_FLOAT, 32) => Encoding::Float32,
        _ => return Err(format(&format!("WAV format {tag} with {bits} bits"))),
    };
    Ok(Pcm {
        rate_hz,
        channels,
        encoding,
        big_endian: false,
        data,
    })
}

fn aiff(bytes: &[u8]) -> Result<Pcm<'_>, AudioFileError> {
    let compressed = match bytes.get(8..12) {
        Some(b"AIFF") => false,
        Some(b"AIFC") => true,
        _ => return Err(format("a FORM file that is not AIFF")),
    };
    let found = chunks(bytes, true);
    let common = found
        .iter()
        .find(|(id, _)| id == b"COMM")
        .map(|(_, body)| *body)
        .ok_or_else(|| format("AIFF without a COMM chunk"))?;
    let sound = found
        .iter()
        .find(|(id, _)| id == b"SSND")
        .map(|(_, body)| *body)
        .ok_or_else(|| format("AIFF without an SSND chunk"))?;
    let short = || format("short COMM chunk");
    let channels = u16_at(common, 0, true).ok_or_else(short)?;
    let bits = u16_at(common, 6, true).ok_or_else(short)?;
    let rate: [u8; 10] = common
        .get(8..18)
        .and_then(|rate| rate.try_into().ok())
        .ok_or_else(short)?;
    let rate_hz = extended_to_hz(rate).ok_or_else(|| format("AIFF sample rate"))?;
    let (encoding, big_endian) = match (compressed, common.get(18..22)) {
        (false, _) | (true, Some(b"NONE" | b"twos")) => (Encoding::Integer { bits }, true),
        (true, Some(b"sowt")) => (Encoding::Integer { bits }, false),
        (true, Some(b"fl32" | b"FL32")) => (Encoding::Float32, true),
        (true, _) => return Err(format("compressed AIFC")),
    };
    if !(8..=32).contains(&bits) {
        return Err(format(&format!("AIFF with {bits} bits")));
    }
    let offset = u32_at(sound, 0, true).ok_or_else(|| format("short SSND chunk"))? as usize;
    let data = sound
        .get(8 + offset..)
        .ok_or_else(|| format("SSND offset past its end"))?;
    Ok(Pcm {
        rate_hz,
        channels,
        encoding,
        big_endian,
        data,
    })
}

/// AIFF's 80-bit extended float sample rate, rounded to whole hertz.
fn extended_to_hz(bytes: [u8; 10]) -> Option<u32> {
    let exponent = i32::from(u16::from_be_bytes([bytes[0], bytes[1]]) & 0x7fff);
    let mut mantissa = [0_u8; 8];
    mantissa.copy_from_slice(&bytes[2..10]);
    let mantissa = u64::from_be_bytes(mantissa);
    if bytes[0] & 0x80 != 0 || mantissa == 0 {
        return None;
    }
    let rate = mantissa as f64 * 2_f64.powi(exponent - 16_383 - 63);
    (1.0..=f64::from(u32::MAX))
        .contains(&rate)
        .then(|| rate.round() as u32)
}

#[cfg(test)]
mod tests {
    use super::{AudioFileError, decode_audio, extended_to_hz};
    use crate::audio::MIC_RATE_HZ;

    fn wav(rate_hz: u32, channels: u16, samples: &[i16]) -> Vec<u8> {
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut bytes = b"RIFF".to_vec();
        bytes.extend((36 + data.len() as u32).to_le_bytes());
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16_u32.to_le_bytes());
        bytes.extend(1_u16.to_le_bytes());
        bytes.extend(channels.to_le_bytes());
        bytes.extend(rate_hz.to_le_bytes());
        bytes.extend((rate_hz * 2 * u32::from(channels)).to_le_bytes());
        bytes.extend((2 * channels).to_le_bytes());
        bytes.extend(16_u16.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend((data.len() as u32).to_le_bytes());
        bytes.extend(data);
        bytes
    }

    /// 22050 Hz as AIFF writes it.
    const RATE_22050: [u8; 10] = [0x40, 0x0d, 0xac, 0x44, 0, 0, 0, 0, 0, 0];

    fn aiff(samples: &[i16]) -> Vec<u8> {
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_be_bytes()).collect();
        let mut common = 1_u16.to_be_bytes().to_vec();
        common.extend((samples.len() as u32).to_be_bytes());
        common.extend(16_u16.to_be_bytes());
        common.extend(RATE_22050);
        let mut bytes = b"FORM".to_vec();
        bytes.extend((4 + 8 + 18 + 8 + 8 + data.len() as u32).to_be_bytes());
        bytes.extend(b"AIFFCOMM");
        bytes.extend(18_u32.to_be_bytes());
        bytes.extend(common);
        bytes.extend(b"SSND");
        bytes.extend((8 + data.len() as u32).to_be_bytes());
        bytes.extend([0; 8]);
        bytes.extend(data);
        bytes
    }

    #[test]
    fn a_16_khz_wav_is_read_as_is_and_stereo_is_averaged() {
        let mono = decode_audio(&wav(MIC_RATE_HZ, 1, &[0, 16_384, -16_384])).expect("decodes");
        assert_eq!(mono, vec![0.0, 0.5, -0.5]);
        let stereo = decode_audio(&wav(MIC_RATE_HZ, 2, &[16_384, 0, -32_768, 0])).expect("decodes");
        assert_eq!(stereo, vec![0.25, -0.5]);
    }

    #[test]
    fn an_aiff_at_22050_hz_is_converted_to_16_khz() {
        assert_eq!(extended_to_hz(RATE_22050), Some(22_050));
        let second = vec![8_192_i16; 22_050];
        let audio = decode_audio(&aiff(&second)).expect("decodes");
        // One second in, one second out (the filter's tail included).
        let expected = MIC_RATE_HZ as usize;
        assert!(
            audio.len().abs_diff(expected) < 32,
            "{} frames",
            audio.len()
        );
        // A constant level stays that level once the filter has settled.
        let middle = audio[audio.len() / 2];
        assert!((middle - 0.25).abs() < 0.01, "{middle}");
    }

    #[test]
    fn other_files_are_refused_with_a_reason() {
        assert!(matches!(
            decode_audio(b"OggS not audio we read"),
            Err(AudioFileError::Format { .. })
        ));
        let mut truncated = wav(MIC_RATE_HZ, 1, &[1, 2, 3]);
        truncated.truncate(20);
        assert!(matches!(
            decode_audio(&truncated),
            Err(AudioFileError::Format { .. })
        ));
    }
}
