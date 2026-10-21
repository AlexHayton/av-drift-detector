//! Streaming synthesis of the audio code.

use super::code::ToneCode;

/// Generates the coded audio stream sample by sample.
///
/// With a positive `delay`, output sample `s` carries the signal for source
/// time `s / rate − delay`: the audio arrives late relative to the video, which
/// the analyser reports as positive drift. Before source time zero the output
/// is silent.
pub struct ToneSynth {
    code: ToneCode,
    delay: f64,
    next_sample: u64,
}

impl ToneSynth {
    pub fn new(code: ToneCode, delay_seconds: f64) -> Self {
        ToneSynth {
            code,
            delay: delay_seconds,
            next_sample: 0,
        }
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
            *sample = self.code.signal_at(t) as f32;
        }
        self.next_sample += out.len() as u64;
    }
}

/// Synthesises `count` samples starting at sample `start` (handy for tests).
pub fn synthesize(code: &ToneCode, delay_seconds: f64, start: u64, count: usize) -> Vec<f32> {
    let mut synth = ToneSynth {
        code: code.clone(),
        delay: delay_seconds,
        next_sample: start,
    };
    let mut out = vec![0.0; count];
    synth.fill(&mut out);
    out
}
