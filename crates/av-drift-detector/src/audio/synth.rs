//! Streaming synthesis of the audio code, and the music under it.

use super::code::ToneCode;
use super::music::Music;

/// Generates the coded audio stream sample by sample.
///
/// With a positive `delay`, output sample `s` carries the signal for source
/// time `s / rate − delay`: the audio arrives late relative to the video, which
/// the analyser reports as positive drift. Before source time zero the output
/// is silent.
pub struct ToneSynth {
    code: ToneCode,
    music: Option<Music>,
    delay: f64,
    next_sample: u64,
}

impl ToneSynth {
    pub fn new(code: ToneCode, delay_seconds: f64) -> Self {
        ToneSynth {
            code,
            music: None,
            delay: delay_seconds,
            next_sample: 0,
        }
    }

    /// Mixes in the [`Music`], where the frame rate leaves room for it (up to
    /// about 60 fps). It doesn't affect decoding.
    pub fn with_music(mut self) -> Self {
        self.music = Music::new(&self.code);
        self
    }

    /// Starts at output sample `sample` instead of 0.
    pub fn starting_at(mut self, sample: u64) -> Self {
        self.next_sample = sample;
        self
    }

    /// Whether music is mixed in.
    pub fn has_music(&self) -> bool {
        self.music.is_some()
    }

    /// Index of the next sample `fill` will produce.
    pub fn position(&self) -> u64 {
        self.next_sample
    }

    /// Writes the next `out.len()` samples.
    pub fn fill(&mut self, out: &mut [f32]) {
        let rate = f64::from(self.code.sample_rate());
        for (i, sample) in out.iter_mut().enumerate() {
            let t = (self.next_sample + i as u64) as f64 / rate - self.delay;
            let music = self.music.as_ref().map_or(0.0, |m| m.signal_at(t));
            *sample = (self.code.signal_at(t) + music) as f32;
        }
        self.next_sample += out.len() as u64;
    }
}

/// Synthesises `count` samples of the code alone, starting at sample `start`
/// (handy for tests).
pub fn synthesize(code: &ToneCode, delay_seconds: f64, start: u64, count: usize) -> Vec<f32> {
    let mut synth = ToneSynth::new(code.clone(), delay_seconds).starting_at(start);
    let mut out = vec![0.0; count];
    synth.fill(&mut out);
    out
}
