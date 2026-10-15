//! A rational sample-rate converter for the ring boundaries (LLD §3.4).
//!
//! VoiceProcessingIO only runs at its own client rate (44.1 kHz on the machines we measured;
//! asking for 16 or 24 kHz makes `AudioUnitInitialize` fail), so capture is converted to the mic
//! ring's 16 kHz and the voice ring's 24 kHz is converted to the unit's rate. The same converter
//! turns the system voice's 22.05 kHz into 24 kHz.
//!
//! It is a polyphase windowed-sinc filter: up by `L`, low-pass, down by `M` (the reduced ratio).
//! All tables are built in [`Resampler::new`]; [`Resampler::push`] and [`Resampler::pull`]
//! never allocate, so they run on the real-time thread. Linear interpolation was rejected: going
//! from 44.1 to 16 kHz it folds everything between 8 and 22 kHz back into the speech band.

use std::f64::consts::PI;

use crate::limits::RESAMPLER_PHASES_MAX;

/// Zero crossings of the sinc kept on each side of the center, at the narrower of the two
/// rates. Eight keeps the passband flat to about 90% of the new Nyquist frequency.
const ZERO_CROSSINGS: usize = 8;

/// The filter's cutoff as a fraction of the lower Nyquist frequency: room for the transition
/// band so it does not alias.
const CUTOFF_FRACTION: f64 = 0.9;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResamplerError {
    /// A rate of zero.
    ZeroRate,
    /// The reduced ratio needs more than [`RESAMPLER_PHASES_MAX`] phases.
    UnsupportedRatio { input_hz: u32, output_hz: u32 },
}

#[derive(Clone, Debug)]
pub struct Resampler {
    up: u32,
    down: u32,
    taps: usize,
    /// `up` rows of `taps` coefficients, each row reversed so it lines up with the history
    /// window (oldest first).
    coefficients: Box<[f32]>,
    /// The last `taps` inputs, stored twice so the window is always one contiguous slice.
    history: Box<[f32]>,
    /// Where the newest input goes next.
    write_index: usize,
    /// Phase of the next output, in `[0, up)`.
    phase: u32,
    /// Inputs still to be pushed before the next output can be computed.
    inputs_needed: u32,
}

impl Resampler {
    pub fn new(input_hz: u32, output_hz: u32) -> Result<Self, ResamplerError> {
        if input_hz == 0 || output_hz == 0 {
            return Err(ResamplerError::ZeroRate);
        }
        let divisor = gcd(input_hz, output_hz);
        let (up, down) = (output_hz / divisor, input_hz / divisor);
        if up > RESAMPLER_PHASES_MAX {
            return Err(ResamplerError::UnsupportedRatio {
                input_hz,
                output_hz,
            });
        }
        let taps = if up == down {
            1
        } else {
            2 * ZERO_CROSSINGS * (down as usize).div_ceil(up as usize).max(1)
        };
        Ok(Self {
            up,
            down,
            taps,
            coefficients: coefficients(up, down, taps),
            history: vec![0.0; 2 * taps].into_boxed_slice(),
            write_index: 0,
            phase: 0,
            inputs_needed: 1,
        })
    }

    /// Delay the filter adds, in input frames (half its length).
    #[must_use]
    pub fn delay_input_frames(&self) -> u32 {
        u32::try_from(self.taps / 2).unwrap_or(u32::MAX)
    }

    /// Converts `input`, handing each output frame to `emit`.
    pub fn push(&mut self, input: &[f32], mut emit: impl FnMut(f32)) {
        for sample in input {
            self.push_input(*sample);
            while self.inputs_needed == 0 {
                emit(self.output());
                self.advance();
            }
        }
    }

    /// Fills `out`, taking input frames from `source` as they are needed.
    pub fn pull(&mut self, out: &mut [f32], mut source: impl FnMut() -> f32) {
        for frame in out {
            while self.inputs_needed > 0 {
                self.push_input(source());
            }
            *frame = self.output();
            self.advance();
        }
    }

    fn push_input(&mut self, sample: f32) {
        self.history[self.write_index] = sample;
        self.history[self.write_index + self.taps] = sample;
        self.write_index = (self.write_index + 1) % self.taps;
        self.inputs_needed -= 1;
    }

    fn output(&self) -> f32 {
        // After a push at `write_index - 1`, the window `[write_index, write_index + taps)`
        // holds the inputs oldest first.
        let window = &self.history[self.write_index..self.write_index + self.taps];
        let row_begin = self.phase as usize * self.taps;
        let row = &self.coefficients[row_begin..row_begin + self.taps];
        row.iter().zip(window).map(|(c, x)| c * x).sum()
    }

    fn advance(&mut self) {
        let next = self.phase + self.down;
        self.inputs_needed += next / self.up;
        self.phase = next % self.up;
    }
}

const fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        let remainder = a % b;
        a = b;
        b = remainder;
    }
    a
}

/// The polyphase table: a Blackman-windowed sinc at `up` times the input rate, cut at the lower
/// of the two Nyquist frequencies, split into `up` rows normalized to unit gain.
fn coefficients(up: u32, down: u32, taps: usize) -> Box<[f32]> {
    if up == down {
        return vec![1.0].into_boxed_slice();
    }
    let up_count = up as usize;
    let length = taps * up_count;
    let cutoff = CUTOFF_FRACTION * 0.5 / f64::from(up.max(down));
    let center = (length as f64 - 1.0) / 2.0;
    let prototype: Vec<f64> = (0..length)
        .map(|index| {
            let time = index as f64 - center;
            let sinc = if time.abs() < 1e-12 {
                2.0 * cutoff
            } else {
                (2.0 * PI * cutoff * time).sin() / (PI * time)
            };
            let position = index as f64 / (length as f64 - 1.0);
            let window =
                0.42 - 0.5 * (2.0 * PI * position).cos() + 0.08 * (4.0 * PI * position).cos();
            sinc * window
        })
        .collect();
    let mut table = vec![0.0_f32; length];
    for phase in 0..up_count {
        let row: Vec<f64> = (0..taps)
            .map(|tap| prototype[phase + tap * up_count])
            .collect();
        let sum: f64 = row.iter().sum();
        for (tap, value) in row.iter().enumerate() {
            // Reversed, so the newest input meets coefficient 0.
            table[phase * taps + (taps - 1 - tap)] = (value / sum) as f32;
        }
    }
    table.into_boxed_slice()
}

#[cfg(test)]
mod tests {
    use super::{Resampler, ResamplerError};

    fn sine(frequency_hz: f64, rate_hz: u32, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|index| {
                (2.0 * std::f64::consts::PI * frequency_hz * index as f64 / f64::from(rate_hz))
                    .sin() as f32
            })
            .collect()
    }

    fn convert(input_hz: u32, output_hz: u32, input: &[f32]) -> Vec<f32> {
        let mut resampler = Resampler::new(input_hz, output_hz).expect("supported ratio");
        let mut out = Vec::new();
        resampler.push(input, |sample| out.push(sample));
        out
    }

    fn rms(samples: &[f32]) -> f64 {
        let sum: f64 = samples.iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
        (sum / samples.len() as f64).sqrt()
    }

    /// Amplitude of `frequency_hz` in `samples`, by correlating with a sine and a cosine.
    fn tone_level(samples: &[f32], frequency_hz: f64, rate_hz: u32) -> f64 {
        let (mut sine_sum, mut cosine_sum) = (0.0, 0.0);
        for (index, sample) in samples.iter().enumerate() {
            let angle =
                2.0 * std::f64::consts::PI * frequency_hz * index as f64 / f64::from(rate_hz);
            sine_sum += f64::from(*sample) * angle.sin();
            cosine_sum += f64::from(*sample) * angle.cos();
        }
        2.0 * (sine_sum * sine_sum + cosine_sum * cosine_sum).sqrt() / samples.len() as f64
    }

    #[test]
    fn output_length_follows_the_ratio() {
        for (input_hz, output_hz) in [(44_100, 16_000), (24_000, 44_100), (48_000, 16_000)] {
            let out = convert(input_hz, output_hz, &vec![0.0; input_hz as usize]);
            let expected = output_hz as usize;
            assert!(
                out.len().abs_diff(expected) <= 1,
                "{input_hz}->{output_hz}: {}",
                out.len()
            );
        }
    }

    #[test]
    fn a_tone_in_the_passband_keeps_its_level() {
        for (input_hz, output_hz) in [(44_100, 16_000), (24_000, 44_100), (22_050, 24_000)] {
            let out = convert(
                input_hz,
                output_hz,
                &sine(1_000.0, input_hz, input_hz as usize),
            );
            let steady = &out[200..out.len() - 200];
            let level = tone_level(steady, 1_000.0, output_hz);
            assert!(
                (level - 1.0).abs() < 0.02,
                "{input_hz}->{output_hz}: {level}"
            );
        }
    }

    #[test]
    fn a_tone_above_the_new_nyquist_is_removed_not_folded() {
        // 11 kHz at 44.1 kHz would fold to 5 kHz at 16 kHz.
        let out = convert(44_100, 16_000, &sine(11_000.0, 44_100, 44_100));
        let steady = &out[200..out.len() - 200];
        let level_db = 20.0 * rms(steady).max(1e-12).log10();
        assert!(level_db < -45.0, "aliasing at {level_db:.1} dB");
    }

    #[test]
    fn pull_and_push_give_the_same_frames() {
        let input = sine(440.0, 24_000, 4_800);
        let pushed = convert(24_000, 44_100, &input);
        let mut resampler = Resampler::new(24_000, 44_100).expect("supported ratio");
        let mut source = input.iter().copied().chain(std::iter::repeat(0.0));
        let mut pulled = vec![0.0; pushed.len()];
        // Pull in uneven blocks, as device cycles do.
        let mut begin = 0;
        for block in [470, 1, 512, 333].iter().cycle() {
            let end = (begin + block).min(pulled.len());
            resampler.pull(&mut pulled[begin..end], || source.next().unwrap_or(0.0));
            begin = end;
            if begin == pulled.len() {
                break;
            }
        }
        assert_eq!(pushed, pulled);
    }

    #[test]
    fn equal_rates_pass_samples_through() {
        let input = sine(440.0, 16_000, 100);
        assert_eq!(convert(16_000, 16_000, &input), input);
    }

    #[test]
    fn unsupported_ratios_are_refused() {
        assert_eq!(
            Resampler::new(0, 16_000).err(),
            Some(ResamplerError::ZeroRate)
        );
        assert!(matches!(
            Resampler::new(16_001, 44_099),
            Err(ResamplerError::UnsupportedRatio { .. })
        ));
    }
}
