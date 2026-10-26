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
//!    gives one nibble of the 16-bit id. A band counts only if its tone is
//!    clean (purity) and clear of the noise, which is measured on the unused
//!    harmonics between the bands. There's no fixed loudness floor: a quiet
//!    recording decodes as long as its tones stand above its own noise.
//!    Consecutive frames carry consecutive ids, so the block votes. Stragglers
//!    are dropped, and a band that some cells lost (a notch in a speaker or a
//!    room, a suppressed tone) is filled in from the others. If a band is lost
//!    throughout a block, the id is known only modulo a power of 16, and the
//!    analyser resolves the rest against the video.
//! 3. **Matched filter.** With the ids known, the block's clean waveform is
//!    known exactly. Cross-correlating it with the recording finds the direct
//!    sound to the nearest sample. Purity alone is pulled late by room echo
//!    (the previous frame's reverb tail lingers into the next); echoes are
//!    weaker and later than the direct sound, so the correlation peak isn't.
//!    The peak's sign also reveals an inverted microphone polarity.
//! 4. **Sub-sample timing.** Each tone's phase at the window start is
//!    `2π·f·(source time)`, so it gives the source time to a small fraction
//!    of a sample. Phases of the readable bands are unwrapped from the lowest
//!    frequency upwards and combined.

use std::collections::HashSet;
use std::f64::consts::TAU;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use super::code::{BANDS, ID_MODULUS, TONE_COUNT, TONES_PER_BAND, ToneCode};

/// Frames aligned together; the boundary phase is assumed constant within a block.
const BLOCK_CELLS: usize = 8;
/// Grid cells decoded per block: one extra before and three after, to cover
/// seams where neighbouring blocks align differently and the end of the buffer.
const GRID_FIRST: i64 = -1;
const GRID_LAST: i64 = BLOCK_CELLS as i64 + 3;
const GRID_CELLS: usize = (GRID_LAST - GRID_FIRST + 1) as usize;
/// Readable bands a cell needs: band 0, which aligns and times it, and one more.
const MIN_BANDS: u32 = 2;
/// Signal-to-noise ratios are capped here, so clean synthetic audio stays finite.
const MAX_SNR_DB: f32 = 120.0;

/// One decoded audio frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioCell {
    /// Index into the analysed buffer where the frame starts (to within a sample).
    pub sample: usize,
    /// Container time of `sample`, in seconds.
    pub time: f64,
    /// The frame id carried by the tones, modulo [`modulus`](Self::modulus).
    pub id: u16,
    /// The id is known modulo this power of 16: 65536 normally, less when a
    /// band was unreadable throughout the block, so its nibble (and those
    /// above it) must come from elsewhere, such as the video.
    pub modulus: u32,
    /// Source time at `time` relative to the start of frame `id`, in seconds.
    /// The audio's source position at `time` is `id_unwrapped / fps + residual`.
    pub residual: f64,
    /// The weakest readable band's tone purity, from 0 to 1.
    pub confidence: f32,
    /// The weakest readable band's tone amplitude (1.0 = full scale).
    pub amplitude: f32,
    /// The weakest readable band's energy above the noise between the bands, in dB.
    pub snr_db: f32,
    /// Which bands were readable (bit `b` for band `b`). The others were
    /// left out of the timing.
    pub bands: u8,
}

/// Detects and decodes [`AudioCell`]s in a mono sample buffer.
pub struct CellDetector {
    code: ToneCode,
    window: usize,
    /// Per-tone reference tables, indexed `band * 16 + tone`.
    cos: Vec<Vec<f32>>,
    sin: Vec<Vec<f32>>,
    /// Reference tables for the unused harmonics between the bands.
    guard_cos: Vec<Vec<f32>>,
    guard_sin: Vec<Vec<f32>>,
    /// A band is unreadable if its tone is quieter than this (1.0 = full
    /// scale). The default, about −110 dBFS, only rejects digital silence:
    /// noise is handled by `min_snr_db` and `min_confidence`, whatever the level.
    pub min_amplitude: f32,
    /// A band is unreadable if its tone purity is below this. Noise alone
    /// scores about 0.2; room echo lowers real cells to around 0.7–0.9.
    pub min_confidence: f32,
    /// A band is unreadable if its tone is less than this many dB above the
    /// noise, measured on the unused harmonics between the bands.
    pub min_snr_db: f32,
    /// Matched filter: block length and lag search range (samples), FFT plans.
    block_span: usize,
    search: usize,
    fft_size: usize,
    fft: Arc<dyn Fft<f32>>,
    ifft: Arc<dyn Fft<f32>>,
}

/// One window's tones, before the block's vote settles its id.
#[derive(Clone, Copy, Debug)]
struct Reading {
    sample: usize,
    /// The strongest tone in each band.
    tones: [usize; BANDS],
    phases: [f64; BANDS],
    amplitudes: [f32; BANDS],
    purity: [f32; BANDS],
    snr_db: [f32; BANDS],
    /// Readable bands, a bit per band. Band 0 always is.
    good: u8,
}

impl Reading {
    fn readable(&self, band: usize) -> bool {
        self.good & (1 << band) != 0
    }
}

/// The block's vote: the id at grid index 0, known modulo `modulus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Vote {
    base: u16,
    modulus: u32,
}

/// Cosine and sine tables of harmonic `k` over one window.
fn reference_tables(code: &ToneCode, k: u32, window: usize) -> (Vec<f32>, Vec<f32>) {
    let w = TAU * f64::from(k) * code.fps().as_f64() / f64::from(code.sample_rate());
    (
        (0..window).map(|n| (w * n as f64).cos() as f32).collect(),
        (0..window).map(|n| (w * n as f64).sin() as f32).collect(),
    )
}

impl CellDetector {
    pub fn new(code: ToneCode) -> Self {
        let window = code.samples_per_frame().round() as usize;
        let (mut cos, mut sin) = (
            Vec::with_capacity(TONE_COUNT),
            Vec::with_capacity(TONE_COUNT),
        );
        for band in 0..BANDS {
            for tone in 0..TONES_PER_BAND {
                let (c, s) = reference_tables(&code, code.harmonic(band, tone), window);
                cos.push(c);
                sin.push(s);
            }
        }
        let (guard_cos, guard_sin) = code
            .guard_harmonics()
            .into_iter()
            .map(|k| reference_tables(&code, k, window))
            .unzip();
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
            guard_cos,
            guard_sin,
            min_amplitude: 10f32.powf(-110.0 / 20.0),
            min_confidence: 0.5,
            min_snr_db: 10.0,
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
    ///
    /// Blocks are read independently, so they're spread across the available cores.
    pub fn detect(&self, samples: &[f32], start_time: f64) -> Vec<AudioCell> {
        let threads = thread::available_parallelism().map_or(1, |n| n.get());
        self.detect_with(samples, start_time, threads)
    }

    /// [`detect`](Self::detect) on up to `threads` threads. The result doesn't
    /// depend on the thread count.
    fn detect_with(&self, samples: &[f32], start_time: f64, threads: usize) -> Vec<AudioCell> {
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
        let blocks = map_in_parallel(&starts, threads, |&q| {
            let phi = self.align_block(samples, q, hop);
            self.decode_block(samples, q + phi, start_time)
        });
        let mut cells: Vec<AudioCell> = blocks.into_iter().flatten().collect();
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
        let readings = self.decode_grid(samples, first, 1.0);
        let Some(v) = vote(&readings) else {
            return Vec::new();
        };
        let agreeing = readings
            .iter()
            .filter(|(j, r)| consistent(v.base, *j, r))
            .map(|(j, _)| *j);
        let (j_min, j_max) =
            agreeing.fold((i64::MAX, i64::MIN), |(lo, hi), j| (lo.min(j), hi.max(j)));
        let (first, polarity) =
            match self.matched_lag(samples, first, i64::from(v.base), j_min..=j_max) {
                Some((lag, polarity)) => (first + lag, polarity),
                None => (first, 1.0),
            };
        let readings = self.decode_grid(samples, first, polarity);
        let Some(v) = vote(&readings) else {
            return Vec::new();
        };
        readings
            .iter()
            .filter(|(j, r)| consistent(v.base, *j, r))
            .map(|(j, r)| self.cell(r, v.base.wrapping_add(*j as u16), v.modulus, start_time))
            .collect()
    }

    /// Reads the cells on the grid `first + j·spf` for `j` in
    /// `GRID_FIRST..=GRID_LAST` (out-of-range windows are skipped).
    fn decode_grid(&self, samples: &[f32], first: f64, polarity: f32) -> Vec<(i64, Reading)> {
        let spf = self.code.samples_per_frame();
        (GRID_FIRST..=GRID_LAST)
            .filter_map(|j| {
                let p = (first + j as f64 * spf).round();
                (p >= 0.0)
                    .then(|| self.read_cell(samples, p as usize, polarity))
                    .flatten()
                    .map(|r| (j, r))
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
    /// The full waveform's correlation has side peaks a whole cycle of each
    /// tone either side of the true one, which an echo can push over it, so
    /// it's only searched near the coarse peak.
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

    /// Reads the cell starting near sample `p`, if enough of its bands are
    /// clean. `polarity` is -1 for an inverted recording.
    ///
    /// If the measured residual shows `p` is a sample or more off the boundary,
    /// the window is re-centred once and read again (less leakage from the
    /// neighbouring frame).
    fn read_cell(&self, samples: &[f32], p: usize, polarity: f32) -> Option<Reading> {
        let r = self.read_window(samples, p, polarity)?;
        let shift = (self.residual(&r) * f64::from(self.code.sample_rate())).round() as i64;
        if shift == 0 {
            return Some(r);
        }
        let Ok(q) = usize::try_from(p as i64 - shift) else {
            return Some(r);
        };
        let same = |c: &Reading| {
            (0..BANDS).all(|b| !r.readable(b) || (c.readable(b) && c.tones[b] == r.tones[b]))
        };
        Some(
            self.read_window(samples, q, polarity)
                .filter(same)
                .unwrap_or(r),
        )
    }

    /// The tones in the window at `p`, or `None` unless band 0 and at least
    /// one other band are readable.
    fn read_window(&self, samples: &[f32], p: usize, polarity: f32) -> Option<Reading> {
        let x = samples.get(p..p + self.window)?;
        let half_window = self.window as f32 / 2.0;
        let mut guard: Vec<f32> = self
            .guard_cos
            .iter()
            .zip(&self.guard_sin)
            .map(|(c, s)| {
                let (re, im) = dot2(x, c, s);
                re * re + im * im
            })
            .collect();
        guard.sort_by(f32::total_cmp);
        let noise = match guard.len() {
            0 => 0.0,
            n => (guard[(n - 1) / 2] + guard[n / 2]) / 2.0,
        };
        let min_ratio = 10f32.powf(self.min_snr_db / 10.0);
        let max_ratio = 10f32.powf(MAX_SNR_DB / 10.0);
        let mut r = Reading {
            sample: p,
            tones: [0; BANDS],
            phases: [0.0; BANDS],
            amplitudes: [0.0; BANDS],
            purity: [0.0; BANDS],
            snr_db: [0.0; BANDS],
            good: 0,
        };
        for band in 0..BANDS {
            let (mut best, mut best_e, mut total) = (0, 0.0f32, 0.0f32);
            let mut best_re_im = (0.0, 0.0);
            for tone in 0..TONES_PER_BAND {
                let t = band * TONES_PER_BAND + tone;
                let (re, im) = dot2(x, &self.cos[t], &self.sin[t]);
                let e = re * re + im * im;
                total += e;
                if e > best_e {
                    (best, best_e, best_re_im) = (tone, e, (re, im));
                }
            }
            r.tones[band] = best;
            r.amplitudes[band] = best_e.sqrt() / half_window;
            // For x[n] = a·sin(θ + ωn): Σx·cos(ωn) ≈ (aL/2)·sin θ and Σx·sin(ωn) ≈ (aL/2)·cos θ.
            r.phases[band] = f64::from((polarity * best_re_im.0).atan2(polarity * best_re_im.1));
            r.purity[band] = if total > 0.0 { best_e / total } else { 0.0 };
            let ratio = if noise > 0.0 {
                (best_e / noise).min(max_ratio)
            } else if best_e > 0.0 {
                max_ratio
            } else {
                0.0
            };
            r.snr_db[band] = if ratio > 0.0 {
                10.0 * ratio.log10()
            } else {
                -MAX_SNR_DB
            };
            if r.amplitudes[band] >= self.min_amplitude
                && r.purity[band] >= self.min_confidence
                && ratio >= min_ratio
            {
                r.good |= 1 << band;
            }
        }
        (r.readable(0) && r.good.count_ones() >= MIN_BANDS).then_some(r)
    }

    /// Source time at the window start relative to the start of its frame,
    /// from the phases of the readable bands.
    ///
    /// θ = 2π·f·(source time) and f·(frame start) is a whole number of cycles,
    /// so θ/(2πf) is the source time relative to the frame start, modulo 1/f.
    /// Windows sit on frame boundaries to within a sample or two (the matched
    /// filter puts them there), so each band's time is taken as the one within
    /// half a cycle of zero. Unwrapping each band against the lower bands'
    /// estimate instead lets a weak or echo-biased low band pull a stronger
    /// high band onto the wrong cycle, a jump of a whole period (0.4–1.7 ms).
    fn residual(&self, r: &Reading) -> f64 {
        let (mut estimate, mut weight_sum) = (0.0f64, 0.0f64);
        for band in (0..BANDS).filter(|&b| r.readable(b)) {
            let f = self.code.frequency(band, r.tones[band]);
            let mut x = r.phases[band] / (TAU * f);
            x -= (x * f).round() / f;
            let w = (f * f64::from(r.amplitudes[band])).powi(2);
            estimate = (estimate * weight_sum + x * w) / (weight_sum + w);
            weight_sum += w;
        }
        estimate
    }

    /// The decoded cell for reading `r`, whose id the block's vote settled.
    fn cell(&self, r: &Reading, id: u16, modulus: u32, start_time: f64) -> AudioCell {
        let weakest = |v: &[f32; BANDS]| {
            (0..BANDS)
                .filter(|&b| r.readable(b))
                .map(|b| v[b])
                .fold(f32::INFINITY, f32::min)
        };
        AudioCell {
            sample: r.sample,
            time: start_time + r.sample as f64 / f64::from(self.code.sample_rate()),
            id,
            modulus,
            residual: self.residual(r),
            confidence: weakest(&r.purity),
            amplitude: weakest(&r.amplitudes),
            snr_db: weakest(&r.snr_db),
            bands: r.good,
        }
    }
}

/// Whether reading `r` at grid index `j` agrees with id `base` at index 0 in
/// every band it could read.
fn consistent(base: u16, j: i64, r: &Reading) -> bool {
    let tones = ToneCode::tones(base.wrapping_add(j as u16));
    (0..BANDS).all(|b| !r.readable(b) || tones[b] == r.tones[b])
}

/// Consecutive cells carry consecutive ids: votes on the id at grid index 0.
///
/// Each reading proposes every base it's consistent with (16 per unreadable
/// band), and each proposal scores the readings that agree with it. Bases
/// that tie differ only in nibbles no agreeing reading could see, so the id is
/// then known only modulo the lowest of those nibbles. Returns `None` if the
/// readings don't agree.
fn vote(readings: &[(i64, Reading)]) -> Option<Vote> {
    let mut candidates: HashSet<u16> = HashSet::new();
    for (j, r) in readings {
        let low = r.tones[0];
        let unknown: Vec<usize> = (0..BANDS).filter(|&b| !r.readable(b)).collect();
        for combo in 0..16usize.pow(unknown.len() as u32) {
            let mut nibbles: [usize; BANDS] =
                std::array::from_fn(|b| ToneCode::nibble(b, r.tones[b], low));
            for (i, &b) in unknown.iter().enumerate() {
                nibbles[b] = (combo >> (4 * i)) & 0xF;
            }
            candidates.insert(ToneCode::id_from_nibbles(nibbles).wrapping_sub(*j as u16));
        }
    }
    let scored: Vec<(u16, usize)> = candidates
        .into_iter()
        .map(|b| {
            (
                b,
                readings
                    .iter()
                    .filter(|(j, r)| consistent(b, *j, r))
                    .count(),
            )
        })
        .collect();
    let best = scored.iter().map(|s| s.1).max()?;
    if best < 2 && readings.len() != 1 {
        return None;
    }
    let tied = || scored.iter().filter(|s| s.1 == best).map(|s| s.0);
    let base = tied().min()?;
    let differ = tied().fold(0u16, |acc, b| acc | (b ^ base));
    let modulus = if differ == 0 {
        ID_MODULUS
    } else {
        1 << (4 * (differ.trailing_zeros() / 4))
    };
    Some(Vote { base, modulus })
}

/// Sorts cells by position and, of any closer than `min_gap` samples, keeps
/// the best: the most certain id, then the most readable bands, then the best
/// aligned (a misaligned window has lower purity or amplitude).
fn dedup_cells(cells: &mut Vec<AudioCell>, min_gap: f64) {
    cells.sort_by_key(|c| c.sample);
    let rank = |c: &AudioCell| (c.modulus, c.bands.count_ones(), c.confidence * c.amplitude);
    let mut out: Vec<AudioCell> = Vec::with_capacity(cells.len());
    for cell in cells.drain(..) {
        match out.last_mut() {
            Some(last) if ((cell.sample - last.sample) as f64) < min_gap => {
                if rank(&cell).partial_cmp(&rank(last)) == Some(std::cmp::Ordering::Greater) {
                    *last = cell;
                }
            }
            _ => out.push(cell),
        }
    }
    *cells = out;
}

/// Blocks per thread below which another thread isn't worth starting.
const MIN_BLOCKS_PER_THREAD: usize = 4;

/// `items.iter().map(f)` on up to `threads` threads, which take the next
/// item as they finish one. Results are in the order of `items`.
fn map_in_parallel<T: Sync, R: Send>(
    items: &[T],
    threads: usize,
    f: impl Fn(&T) -> R + Sync,
) -> Vec<R> {
    let threads = threads.min(items.len() / MIN_BLOCKS_PER_THREAD);
    if threads <= 1 {
        return items.iter().map(f).collect();
    }
    let next = AtomicUsize::new(0);
    let mut done: Vec<(usize, R)> = thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut out = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(i) else {
                            break out;
                        };
                        out.push((i, f(item)));
                    }
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|w| w.join().unwrap_or_else(|e| std::panic::resume_unwind(e)))
            .collect()
    });
    done.sort_unstable_by_key(|&(i, _)| i);
    done.into_iter().map(|(_, r)| r).collect()
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
    fn threads_dont_change_the_result() {
        let fps: FrameRate = "29.97".parse().unwrap();
        let code = ToneCode::new(fps, 48000).unwrap();
        let mut samples = synthesize(&code, 0.0123, 0, 48000 * 6);
        // A dropout, so some blocks find nothing and finish early.
        samples[48000..60000].fill(0.0);
        let mut seed = 777u32;
        for s in samples.iter_mut() {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            *s += 0.01 * ((seed >> 16) as f32 / 32768.0 - 1.0);
        }
        let detector = CellDetector::new(code);
        let one = detector.detect_with(&samples, 3.0, 1);
        assert!(one.len() > 150, "only {} cells", one.len());
        for threads in [2, 3, 8, 64] {
            assert_eq!(detector.detect_with(&samples, 3.0, threads), one);
        }
    }

    #[test]
    fn parallel_map_keeps_order() {
        let items: Vec<u64> = (0..100).collect();
        // Uneven work, so threads finish out of order.
        let slow = |&i: &u64| (0..(i % 7) * 20_000).fold(i, |a, b| a ^ b.wrapping_mul(31)) ^ i;
        let expected: Vec<u64> = items.iter().map(slow).collect();
        for threads in [1, 4, 16] {
            assert_eq!(map_in_parallel(&items, threads, slow), expected);
        }
        assert!(map_in_parallel(&[] as &[u64], 8, slow).is_empty());
    }

    #[test]
    fn silence_yields_nothing() {
        let code = ToneCode::new("30".parse().unwrap(), 48000).unwrap();
        let detector = CellDetector::new(code);
        assert!(detector.detect(&vec![0.0; 48000], 0.0).is_empty());
    }
}
