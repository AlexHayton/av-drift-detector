//! Finds frame cells in captured audio and decodes them to sub-sample precision.
//!
//! 1. **Alignment.** The low nibble (band 0) changes on every frame. A window
//!    that lines up with a frame holds exactly one band-0 tone; a window that
//!    straddles two frames holds two. Band-0 *purity* (energy in the strongest
//!    tone ÷ energy in all 16) therefore peaks at the frame boundaries. Each
//!    block of frames is aligned by maximising energy-weighted purity over a
//!    coarse grid of boundary phases (circular, modulo one frame), then a
//!    pattern search narrows it to one sample. Energy weighting stops faint
//!    windows (mostly silence) from pulling the alignment off.
//! 2. **Decoding.** At each aligned window, the strongest tone in each band
//!    gives one nibble of the 16-bit id. Consecutive frames must carry
//!    consecutive ids, so the block votes and stragglers are dropped.
//! 3. **Matched filter.** With the ids known, the block's clean waveform is
//!    known exactly. Cross-correlating it with the recording finds the direct
//!    sound to the nearest sample. Purity alone is pulled late by room echo
//!    (the previous frame's reverb tail lingers into the next); echoes are
//!    weaker and later than the direct sound, so the correlation peak isn't.
//!    The peak's sign also reveals an inverted microphone polarity.
//! 4. **Sub-sample timing.** Each tone's phase at the window start is
//!    `2π·f·(source time)`, so it gives the source time to a small fraction
//!    of a sample. Phases are unwrapped from the lowest frequency upwards and
//!    combined.

use std::cmp::Reverse;
use std::collections::HashMap;
use std::f64::consts::TAU;
use std::sync::Arc;

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use super::code::{BANDS, TONE_AMPLITUDE, TONE_COUNT, TONES_PER_BAND, ToneCode};

/// Frames aligned together; the boundary phase is assumed constant within a block.
const BLOCK_CELLS: usize = 8;
/// Grid cells decoded per block: one extra before and three after, to cover
/// seams where neighbouring blocks align differently and the end of the buffer.
const GRID_FIRST: i64 = -1;
const GRID_LAST: i64 = BLOCK_CELLS as i64 + 3;
const GRID_CELLS: usize = (GRID_LAST - GRID_FIRST + 1) as usize;

/// One decoded audio frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioCell {
    /// Index into the analysed buffer where the frame starts (to within a sample).
    pub sample: usize,
    /// Container time of `sample`, in seconds.
    pub time: f64,
    /// The 16-bit frame id carried by the tones.
    pub id: u16,
    /// Source time at `time` relative to the start of frame `id`, in seconds.
    /// The audio's source position at `time` is `id_unwrapped / fps + residual`.
    pub residual: f64,
    /// The weakest band's tone purity, from 0 to 1.
    pub confidence: f32,
    /// The weakest band's tone amplitude (1.0 = full scale).
    pub amplitude: f32,
}

/// Detects and decodes [`AudioCell`]s in a mono sample buffer.
pub struct CellDetector {
    code: ToneCode,
    window: usize,
    /// Per-tone reference tables, indexed `band * 16 + nibble`.
    cos: Vec<Vec<f32>>,
    sin: Vec<Vec<f32>>,
    /// Cells quieter than this (in any band) are ignored. The default, 40 dB
    /// below a generated tone (-62 dBFS), suits quiet microphone recordings;
    /// random noise is rejected by purity instead.
    pub min_amplitude: f32,
    /// Cells whose weakest band purity is below this are ignored. Noise alone
    /// scores about 0.2; room echo lowers real cells to around 0.7–0.9.
    pub min_confidence: f32,
    /// Matched filter: block length and lag search range (samples), FFT plans.
    block_span: usize,
    search: usize,
    fft_size: usize,
    fft: Arc<dyn Fft<f32>>,
    ifft: Arc<dyn Fft<f32>>,
}

impl CellDetector {
    pub fn new(code: ToneCode) -> Self {
        let window = code.samples_per_frame().round() as usize;
        let rate = f64::from(code.sample_rate());
        let (mut cos, mut sin) = (
            Vec::with_capacity(TONE_COUNT),
            Vec::with_capacity(TONE_COUNT),
        );
        for band in 0..BANDS {
            for nibble in 0..TONES_PER_BAND {
                let w = TAU * code.frequency(band, nibble) / rate;
                cos.push((0..window).map(|n| (w * n as f64).cos() as f32).collect());
                sin.push((0..window).map(|n| (w * n as f64).sin() as f32).collect());
            }
        }
        // Room for the longest template: every cell `decode_grid` can return.
        let block_span = (GRID_CELLS as f64 * code.samples_per_frame()).ceil() as usize + 1;
        let search = window / 2;
        let fft_size = (block_span + 2 * search).next_power_of_two();
        let mut planner = FftPlanner::new();
        let (fft, ifft) = (
            planner.plan_fft_forward(fft_size),
            planner.plan_fft_inverse(fft_size),
        );
        CellDetector {
            code,
            window,
            cos,
            sin,
            min_amplitude: (TONE_AMPLITUDE / 100.0) as f32,
            min_confidence: 0.5,
            block_span,
            search,
            fft_size,
            fft,
            ifft,
        }
    }

    pub fn code(&self) -> &ToneCode {
        &self.code
    }

    /// Finds every decodable frame cell in `samples`, whose first sample is at
    /// container time `start_time`. Cells are returned in time order.
    pub fn detect(&self, samples: &[f32], start_time: f64) -> Vec<AudioCell> {
        let spf = self.code.samples_per_frame();
        let hop = (self.window / 16).max(1) as f64;
        let len = samples.len() as f64;
        let span = (BLOCK_CELLS + 2) as f64 * spf + 2.0 * self.window as f64;
        let mut starts = Vec::new();
        let mut q = 0.0;
        while q + span <= len {
            starts.push(q);
            q += BLOCK_CELLS as f64 * spf;
        }
        // One last block flush with the end, so the tail of the buffer isn't skipped.
        if let Some(&last) = starts.last()
            && len - span > last
        {
            starts.push(len - span);
        }
        let mut cells = Vec::new();
        for q in starts {
            let phi = self.align_block(samples, q, hop);
            cells.extend(self.decode_block(samples, q + phi, start_time));
        }
        dedup_cells(&mut cells, spf / 2.0);
        cells
    }

    /// Returns the cell-boundary phase (offset from `q`, in `[0, spf)`) for the
    /// block starting at `q`. The phase is circular: stepping below zero wraps
    /// to the equivalent boundary one frame later.
    fn align_block(&self, samples: &[f32], q: f64, hop: f64) -> f64 {
        let spf = self.code.samples_per_frame();
        let score = |phi: f64| {
            // Energy-weighted, so a faint window (e.g. mostly silence) can't dominate.
            let (best, total) = (0..BLOCK_CELLS)
                .map(|j| self.band0_energy(samples, (q + phi + j as f64 * spf).round() as usize))
                .fold((0.0, 0.0), |(b, t), (eb, et)| (b + eb, t + et));
            if total > 0.0 { best / total } else { 0.0 }
        };

        let (mut phi, mut best) = (0.0, f32::MIN);
        let mut candidate = 0.0;
        while candidate < spf {
            let s = score(candidate);
            if s > best {
                (phi, best) = (candidate, s);
            }
            candidate += hop;
        }
        let mut step = (hop / 2.0).floor();
        while step >= 1.0 {
            let mut moved = false;
            for c in [phi - step, phi + step] {
                let c = f64::rem_euclid(c, spf);
                let s = score(c);
                if s > best {
                    (phi, best, moved) = (c, s, true);
                }
            }
            if !moved {
                step = (step / 2.0).floor();
            }
        }
        phi
    }

    /// Decodes the cells of one aligned block, keeping those whose ids agree.
    fn decode_block(&self, samples: &[f32], first: f64, start_time: f64) -> Vec<AudioCell> {
        let decoded = self.decode_grid(samples, first, 1.0, start_time);
        let Some(base) = vote(&decoded) else {
            return Vec::new();
        };
        let agreeing = decoded
            .iter()
            .filter(|(j, c)| grid_base(*j, c) == base)
            .map(|(j, _)| *j);
        let (j_min, j_max) =
            agreeing.fold((i64::MAX, i64::MIN), |(lo, hi), j| (lo.min(j), hi.max(j)));
        let (first, polarity) = match self.matched_lag(samples, first, base, j_min..=j_max) {
            Some((lag, polarity)) => (first + lag, polarity),
            None => (first, 1.0),
        };
        let decoded = self.decode_grid(samples, first, polarity, start_time);
        let Some(base) = vote(&decoded) else {
            return Vec::new();
        };
        decoded
            .into_iter()
            .filter(|(j, c)| grid_base(*j, c) == base)
            .map(|(_, c)| c)
            .collect()
    }

    /// Decodes the cells on the grid `first + j·spf` for `j` in
    /// `GRID_FIRST..=GRID_LAST` (out-of-range windows are skipped).
    fn decode_grid(
        &self,
        samples: &[f32],
        first: f64,
        polarity: f32,
        start_time: f64,
    ) -> Vec<(i64, AudioCell)> {
        let spf = self.code.samples_per_frame();
        (GRID_FIRST..=GRID_LAST)
            .filter_map(|j| {
                let p = (first + j as f64 * spf).round();
                (p >= 0.0)
                    .then(|| self.decode_cell(samples, p as usize, polarity, start_time))
                    .flatten()
                    .map(|c| (j, c))
            })
            .collect()
    }

    /// Cross-correlates the recording around grid cells `cells` (grid starting
    /// at `first`, cell 0 carrying id `base`) with their clean waveform.
    /// Returns the correction to `first` in samples, and the polarity.
    ///
    /// Two passes. Coarse: the envelope of the correlation with band 0 alone.
    /// Band 0 steps to a new tone every frame, so this has a single clean
    /// peak with weak side lobes, and a later, weaker echo can't win. Fine:
    /// the full waveform near that peak, which pins the direct sound to a sample.
    /// The full waveform alone can't be trusted over the whole range: bands 1–3
    /// hold steady tones across a block, giving strong side peaks that an echo
    /// can push over the true one.
    fn matched_lag(
        &self,
        samples: &[f32],
        first: f64,
        base: i64,
        cells: std::ops::RangeInclusive<i64>,
    ) -> Option<(f64, f32)> {
        let spf = self.code.samples_per_frame();
        let (search, n) = (self.search, self.fft_size);
        let origin = first + *cells.start() as f64 * spf;
        let span = ((cells.end() - cells.start() + 1) as f64 * spf).round() as usize;
        let span = span.min(self.block_span);
        let start = origin.round() as i64 - search as i64;
        let rate = f64::from(self.code.sample_rate());
        // Ids wrap at 2^16; keep the template's source time non-negative.
        let first_id = (base + cells.start()).rem_euclid(1 << 16);
        let t0 = self.code.fps().frame_time(first_id);

        // Zero-padded beyond the buffer, so blocks at the very start or end are refined too.
        let mut x: Vec<Complex<f32>> = vec![Complex::default(); n];
        for (i, dst) in x.iter_mut().take(span + 2 * search).enumerate() {
            if let Some(&v) = usize::try_from(start + i as i64)
                .ok()
                .and_then(|k| samples.get(k))
            {
                dst.re = v;
            }
        }
        let mut full: Vec<Complex<f32>> = vec![Complex::default(); n];
        let mut band0: Vec<Complex<f32>> = vec![Complex::default(); n];
        for i in 0..span {
            let t = t0 + i as f64 / rate;
            full[i].re = self.code.signal_at(t) as f32;
            if let Some(phase) = self.code.band_phase(0, t) {
                band0[i] = Complex::new(phase.cos() as f32, phase.sin() as f32);
            }
        }
        self.fft.process(&mut x);
        self.fft.process(&mut full);
        self.fft.process(&mut band0);
        for template in [&mut full, &mut band0] {
            for (t, recording) in template.iter_mut().zip(&x) {
                *t = recording * t.conj();
            }
            self.ifft.process(template);
        }
        // Each is now Σₙ recording[start + l + n]·template*[n] at index l
        // (no wrap-around for l ≤ 2·search).
        let coarse = (0..=2 * search)
            .max_by(|&a, &b| band0[a].norm_sqr().total_cmp(&band0[b].norm_sqr()))?;
        let radius = ((0.0015 * rate) as usize).clamp(1, (search / 2).max(1));
        let range = coarse.saturating_sub(radius)..=(coarse + radius).min(2 * search);
        let (lag, peak) = range
            .map(|l| (l, full[l].re))
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))?;
        if peak == 0.0 {
            return None;
        }
        let polarity = if peak < 0.0 { -1.0 } else { 1.0 };
        Some(((start + lag as i64) as f64 - origin, polarity))
    }

    /// Band-0 energy as `(strongest tone, all tones)` for the window at `p`.
    fn band0_energy(&self, samples: &[f32], p: usize) -> (f32, f32) {
        let Some(x) = samples.get(p..p + self.window) else {
            return (0.0, 0.0);
        };
        let (mut best, mut total) = (0.0f32, 0.0f32);
        for tone in 0..TONES_PER_BAND {
            let (re, im) = dot2(x, &self.cos[tone], &self.sin[tone]);
            let e = re * re + im * im;
            total += e;
            best = best.max(e);
        }
        (best, total)
    }

    /// Decodes the frame cell starting near sample `p`, if it is loud and clean
    /// enough. `polarity` is -1 for an inverted recording.
    ///
    /// If the measured residual shows `p` is a sample or more off the boundary,
    /// the window is re-centred once and decoded again (less leakage from the
    /// neighbouring frame).
    fn decode_cell(
        &self,
        samples: &[f32],
        p: usize,
        polarity: f32,
        start_time: f64,
    ) -> Option<AudioCell> {
        let cell = self.decode_window(samples, p, polarity, start_time)?;
        let shift = (cell.residual * f64::from(self.code.sample_rate())).round() as i64;
        if shift == 0 {
            return Some(cell);
        }
        let q = usize::try_from(p as i64 - shift).ok()?;
        self.decode_window(samples, q, polarity, start_time)
            .filter(|c| c.id == cell.id)
            .or(Some(cell))
    }

    fn decode_window(
        &self,
        samples: &[f32],
        p: usize,
        polarity: f32,
        start_time: f64,
    ) -> Option<AudioCell> {
        let x = samples.get(p..p + self.window)?;
        let half_window = self.window as f32 / 2.0;
        let mut nibbles = [0usize; BANDS];
        let mut phases = [0.0f64; BANDS];
        let mut amplitudes = [0.0f32; BANDS];
        let mut confidence = 1.0f32;
        for band in 0..BANDS {
            let (mut best, mut best_e, mut total) = (0, -1.0f32, 0.0f32);
            let mut best_re_im = (0.0, 0.0);
            for nibble in 0..TONES_PER_BAND {
                let tone = band * TONES_PER_BAND + nibble;
                let (re, im) = dot2(x, &self.cos[tone], &self.sin[tone]);
                let e = re * re + im * im;
                total += e;
                if e > best_e {
                    (best, best_e, best_re_im) = (nibble, e, (re, im));
                }
            }
            nibbles[band] = best;
            amplitudes[band] = best_e.max(0.0).sqrt() / half_window;
            // For x[n] = a·sin(θ + ωn): Σx·cos(ωn) ≈ (aL/2)·sin θ and Σx·sin(ωn) ≈ (aL/2)·cos θ.
            phases[band] = f64::from((polarity * best_re_im.0).atan2(polarity * best_re_im.1));
            confidence = confidence.min(if total > 0.0 { best_e / total } else { 0.0 });
        }
        let amplitude = amplitudes.iter().copied().fold(f32::MAX, f32::min);
        if amplitude < self.min_amplitude || confidence < self.min_confidence {
            return None;
        }

        // θ = 2π·f·(source time) and f·(frame start) is a whole number of cycles,
        // so θ/(2πf) is the source time relative to the frame start, modulo 1/f.
        let (mut estimate, mut weight_sum) = (0.0f64, 0.0f64);
        for band in 0..BANDS {
            let f = self.code.frequency(band, nibbles[band]);
            let mut r = phases[band] / (TAU * f);
            if weight_sum > 0.0 {
                r += ((estimate - r) * f).round() / f;
            }
            let w = (f * f64::from(amplitudes[band])).powi(2);
            estimate = (estimate * weight_sum + r * w) / (weight_sum + w);
            weight_sum += w;
        }

        Some(AudioCell {
            sample: p,
            time: start_time + p as f64 / f64::from(self.code.sample_rate()),
            id: ToneCode::id_from_nibbles(nibbles),
            residual: estimate,
            confidence,
            amplitude,
        })
    }
}

/// Consecutive cells carry consecutive ids: votes on the id at grid index 0.
/// Returns it, or `None` if the cells don't agree.
fn vote(decoded: &[(i64, AudioCell)]) -> Option<i64> {
    let mut votes: HashMap<i64, usize> = HashMap::new();
    for (j, cell) in decoded {
        *votes.entry(grid_base(*j, cell)).or_default() += 1;
    }
    let (&winner, &count) = votes.iter().max_by_key(|&(&b, &n)| (n, Reverse(b)))?;
    (count > 1 || decoded.len() == 1).then_some(winner)
}

/// The id that grid cell 0 would carry, according to cell `j`.
fn grid_base(j: i64, cell: &AudioCell) -> i64 {
    (i64::from(cell.id) - j).rem_euclid(1 << 16)
}

/// Sorts cells by position and, of any closer than `min_gap` samples, keeps
/// the best aligned (a misaligned window has lower purity or amplitude).
fn dedup_cells(cells: &mut Vec<AudioCell>, min_gap: f64) {
    cells.sort_by_key(|c| c.sample);
    let mut out: Vec<AudioCell> = Vec::with_capacity(cells.len());
    for cell in cells.drain(..) {
        match out.last_mut() {
            Some(last) if ((cell.sample - last.sample) as f64) < min_gap => {
                if cell.confidence * cell.amplitude > last.confidence * last.amplitude {
                    *last = cell;
                }
            }
            _ => out.push(cell),
        }
    }
    *cells = out;
}

/// `(Σ x·c, Σ x·s)` with eight independent accumulators so the loop vectorises.
fn dot2(x: &[f32], c: &[f32], s: &[f32]) -> (f32, f32) {
    let (mut ac, mut as_) = ([0.0f32; 8], [0.0f32; 8]);
    let (xs, rest) = x.as_chunks::<8>();
    let chunks = xs
        .iter()
        .zip(c.as_chunks::<8>().0)
        .zip(s.as_chunks::<8>().0);
    for ((xs, cs), ss) in chunks {
        for i in 0..8 {
            ac[i] += xs[i] * cs[i];
            as_[i] += xs[i] * ss[i];
        }
    }
    let tail = x.len() - rest.len();
    let (mut re, mut im) = (ac.iter().sum::<f32>(), as_.iter().sum::<f32>());
    for i in tail..x.len() {
        re += x[i] * c[i];
        im += x[i] * s[i];
    }
    (re, im)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FrameRate;
    use crate::audio::synth::synthesize;

    /// Checks that every decoded cell reports the true source time to within `tol` seconds.
    fn check(fps: &str, rate: u32, delay: f64, noise: f32, tol: f64) {
        let fps: FrameRate = fps.parse().unwrap();
        let code = ToneCode::new(fps, rate).unwrap();
        let mut samples = synthesize(&code, delay, 0, rate as usize * 2);
        let mut seed = 12345u32;
        for s in samples.iter_mut() {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            *s += noise * ((seed >> 16) as f32 / 32768.0 - 1.0);
        }
        let detector = CellDetector::new(code);
        let start_time = 10.0;
        let cells = detector.detect(&samples, start_time);
        let expected = ((2.0 - delay.max(0.0)) * fps.as_f64()) as usize;
        assert!(
            cells.len() + 12 >= expected,
            "{fps}: only {} of ~{expected} cells",
            cells.len()
        );
        for pair in cells.windows(2) {
            assert_eq!(
                pair[1].id,
                pair[0].id.wrapping_add(1),
                "{fps}: non-consecutive ids"
            );
        }
        for cell in &cells {
            let source = fps.frame_time(i64::from(cell.id)) + cell.residual;
            let truth = cell.time - start_time - delay;
            assert!(
                (source - truth).abs() < tol,
                "{fps} @ {rate}: cell {} off by {:.3} µs",
                cell.id,
                (source - truth) * 1e6
            );
        }
    }

    #[test]
    fn decodes_exactly_on_clean_audio() {
        for fps in ["24", "25", "29.97", "30", "50", "59.94", "60", "120"] {
            check(fps, 48000, 0.0, 0.0, 2e-6);
            check(fps, 48000, 0.012_345, 0.0, 2e-6);
            check(fps, 48000, -0.2, 0.0, 2e-6);
        }
    }

    #[test]
    fn decodes_at_other_sample_rates() {
        check("30", 44100, 0.0789, 0.0, 2e-6);
        check("29.97", 32000, 0.5, 0.0, 2e-6);
        check("25", 96000, 0.001, 0.0, 2e-6);
    }

    #[test]
    fn tolerates_noise() {
        // Noise at roughly -20 dB relative to each tone.
        check("30", 48000, 0.033, 0.02, 20e-6);
        check("60", 48000, -0.01, 0.02, 20e-6);
    }

    #[test]
    fn handles_inverted_polarity() {
        let fps: FrameRate = "30".parse().unwrap();
        let code = ToneCode::new(fps, 48000).unwrap();
        let samples: Vec<f32> = synthesize(&code, 0.0217, 0, 96000)
            .iter()
            .map(|s| -s)
            .collect();
        let cells = CellDetector::new(code).detect(&samples, 0.0);
        assert!(cells.len() > 50);
        for cell in cells {
            let error = fps.frame_time(i64::from(cell.id)) + cell.residual - (cell.time - 0.0217);
            assert!(
                error.abs() < 2e-6,
                "cell {} off by {:.2} µs",
                cell.id,
                error * 1e6
            );
        }
    }

    #[test]
    fn silence_yields_nothing() {
        let code = ToneCode::new("30".parse().unwrap(), 48000).unwrap();
        let detector = CellDetector::new(code);
        assert!(detector.detect(&vec![0.0; 48000], 0.0).is_empty());
    }
}
