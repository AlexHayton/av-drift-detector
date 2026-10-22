//! The 16-bit audio frame code.
//!
//! During frame `N`, the audio plays four simultaneous tones. Band `b` carries
//! nibble `b` of `N mod 65536`, as one of 16 frequencies. Every frequency is a
//! whole multiple of the frame rate (`f = k·fps`), so each tone completes a
//! whole number of cycles per frame. That has three consequences:
//!
//! * a window aligned to a frame sees each tone exactly on an orthogonal DFT bin
//!   (no leakage between neighbouring frequencies);
//! * every tone is at phase zero at every frame boundary, so the waveform is
//!   continuous with no clicks;
//! * the phase of any tone at sample `t` is `2π·f·t` whatever the frame, so a
//!   tone's phase measures the source time directly, to a fraction of a sample.

use crate::{Error, FrameRate, Result};

pub const BANDS: usize = 4;
pub const TONES_PER_BAND: usize = 16;
/// Unused harmonics between bands.
pub const BAND_GAP: usize = 2;
pub const TONE_COUNT: usize = BANDS * TONES_PER_BAND;
/// Lowest tone frequency (rounded up to a harmonic of the frame rate).
pub const MIN_FREQUENCY_HZ: f64 = 600.0;
/// Peak amplitude of each tone; the four tones peak at 0.32 of full scale
/// together, leaving room for the [`Music`](super::Music) to be heard. (Up
/// to 0.2.1 it was 0.2.) Decoding doesn't depend on the level, short of a
/// recording too quiet to hear.
pub const TONE_AMPLITUDE: f64 = 0.08;
/// Audio frame ids wrap at this value.
pub const ID_MODULUS: u32 = 1 << 16;

/// The tone plan for one frame rate and sample rate.
#[derive(Clone, Debug, PartialEq)]
pub struct ToneCode {
    fps: FrameRate,
    sample_rate: u32,
    base_harmonic: u32,
}

impl ToneCode {
    /// The standard plan for `fps`: the lowest tone is the first harmonic at or above 600 Hz.
    pub fn new(fps: FrameRate, sample_rate: u32) -> Result<Self> {
        Self::with_base_harmonic(fps, sample_rate, Self::default_base_harmonic(fps))
    }

    /// The default base harmonic for `fps`.
    pub fn default_base_harmonic(fps: FrameRate) -> u32 {
        (MIN_FREQUENCY_HZ / fps.as_f64()).ceil() as u32
    }

    /// A plan with an explicit base harmonic, e.g. one read from a QR code.
    pub fn with_base_harmonic(
        fps: FrameRate,
        sample_rate: u32,
        base_harmonic: u32,
    ) -> Result<Self> {
        if base_harmonic == 0 {
            return Err(Error::InvalidOption(
                "tone base harmonic must be positive".into(),
            ));
        }
        let code = ToneCode {
            fps,
            sample_rate,
            base_harmonic,
        };
        let max = code.frequency(BANDS - 1, TONES_PER_BAND - 1);
        if max > 0.45 * f64::from(sample_rate) {
            return Err(Error::InvalidOption(format!(
                "{} fps needs tones up to {max:.0} Hz, too high for a {sample_rate} Hz sample rate",
                fps.label()
            )));
        }
        Ok(code)
    }

    pub fn fps(&self) -> FrameRate {
        self.fps
    }

    pub fn base_harmonic(&self) -> u32 {
        self.base_harmonic
    }

    /// Compact description of the plan for the QR code, e.g. `T4X16G2K21`:
    /// tones, 4 bands × 16 tones, 2-harmonic gaps, base harmonic 21.
    pub fn descriptor(&self) -> String {
        format!(
            "T{BANDS}X{TONES_PER_BAND}G{BAND_GAP}K{}",
            self.base_harmonic
        )
    }

    /// Parses a [`descriptor`](Self::descriptor), returning the base harmonic.
    /// Fails for plans this version can't decode.
    pub fn parse_descriptor(text: &str) -> Result<u32> {
        let unsupported = || Error::Unsupported(format!("audio code `{text}`"));
        let k = text
            .strip_prefix(&format!("T{BANDS}X{TONES_PER_BAND}G{BAND_GAP}K"))
            .ok_or_else(unsupported)?;
        k.parse().ok().filter(|&k| k > 0).ok_or_else(unsupported)
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Exact (possibly fractional) samples per frame.
    pub fn samples_per_frame(&self) -> f64 {
        self.fps.samples_per_frame(self.sample_rate)
    }

    /// Harmonic number `k` (frequency = `k·fps`) of `nibble` in `band`.
    pub fn harmonic(&self, band: usize, nibble: usize) -> u32 {
        self.base_harmonic + (band * (TONES_PER_BAND + BAND_GAP) + nibble) as u32
    }

    pub fn frequency(&self, band: usize, nibble: usize) -> f64 {
        f64::from(self.harmonic(band, nibble)) * self.fps.as_f64()
    }

    /// Splits an id into its four band nibbles (band 0 = least significant).
    pub fn nibbles(id: u16) -> [usize; BANDS] {
        std::array::from_fn(|b| usize::from((id >> (4 * b)) & 0xF))
    }

    pub fn id_from_nibbles(nibbles: [usize; BANDS]) -> u16 {
        nibbles
            .iter()
            .enumerate()
            .fold(0, |id, (b, &n)| id | ((n as u16 & 0xF) << (4 * b)))
    }

    /// The test signal at source time `t` seconds (silence before zero).
    pub fn signal_at(&self, t: f64) -> f64 {
        (0..BANDS)
            .filter_map(|band| self.band_phase(band, t))
            .map(|phase| TONE_AMPLITUDE * phase.sin())
            .sum()
    }

    /// Phase (radians) of `band`'s tone at source time `t`; `None` before zero.
    /// The band's contribution to the signal is `TONE_AMPLITUDE · sin(phase)`.
    pub fn band_phase(&self, band: usize, t: f64) -> Option<f64> {
        if t < 0.0 {
            return None;
        }
        let position = t * self.fps.as_f64();
        let frame = position.floor();
        // Phase within the frame: tone `k` is sin(2π·k·frac), i.e. sin(2π·f·t).
        let frac = position - frame;
        let id = (frame as u64 % u64::from(ID_MODULUS)) as u16;
        let k = f64::from(self.harmonic(band, Self::nibbles(id)[band]));
        Some(std::f64::consts::TAU * k * frac)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nibbles_roundtrip() {
        for id in [0u16, 1, 0xF, 0x10, 0xABCD, 0xFFFF] {
            assert_eq!(ToneCode::id_from_nibbles(ToneCode::nibbles(id)), id);
        }
        assert_eq!(ToneCode::nibbles(0x1234), [4, 3, 2, 1]);
    }

    #[test]
    fn frequency_plan() {
        let code = ToneCode::new("30".parse().unwrap(), 48000).unwrap();
        assert_eq!(code.frequency(0, 0), 600.0);
        assert_eq!(code.frequency(3, 15), 2670.0);
        let code = ToneCode::new("60".parse().unwrap(), 48000).unwrap();
        assert_eq!(code.frequency(3, 15), 4740.0);
        let code = ToneCode::new("120".parse().unwrap(), 48000).unwrap();
        assert_eq!(code.frequency(3, 15), 8880.0);
        assert!(ToneCode::new("120".parse().unwrap(), 16000).is_err());
    }

    #[test]
    fn descriptor_roundtrip() {
        let code = ToneCode::new("29.97".parse().unwrap(), 48000).unwrap();
        assert_eq!(code.descriptor(), "T4X16G2K21");
        assert_eq!(ToneCode::parse_descriptor("T4X16G2K21").unwrap(), 21);
        assert!(ToneCode::parse_descriptor("T4X8G2K21").is_err());
        assert!(ToneCode::parse_descriptor("T4X16G2K0").is_err());
    }

    #[test]
    fn signal_is_continuous_at_frame_boundaries() {
        let code = ToneCode::new("29.97".parse().unwrap(), 48000).unwrap();
        for frame in [1i64, 2, 100, 1000] {
            let t = code.fps().frame_time(frame);
            assert!(code.signal_at(t - 1e-9).abs() < 1e-4);
            assert!(code.signal_at(t + 1e-9).abs() < 1e-4);
        }
        assert_eq!(code.signal_at(-0.5), 0.0);
    }
}
