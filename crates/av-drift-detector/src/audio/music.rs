//! A tune for the dancer, mixed under the code without disturbing it.
//!
//! Every note is a whole multiple of the frame rate, below the lowest code
//! tone, and notes start, stop and change level only at frame boundaries,
//! where every such harmonic is at phase zero (so there are no clicks). Over
//! any one frame the music is then a sum of whole-cycle sinusoids on
//! harmonics the code doesn't use, which a decoding window aligned to that
//! frame doesn't see at all: the music is orthogonal to every code tone.
//!
//! That leaves the notes of the frame rate's harmonic series, in just
//! intonation. With "do" on harmonic 8 (240 Hz at 30 fps) there's do, re, mi,
//! sol, ti and the bluesy natural seventh: enough for an oom-pah vaudeville
//! tune in the dancer's top-hat-and-cane style, 16 beats at 120 beats per
//! minute, looping with his routine every 8 seconds. Bass notes get a few
//! overtones (harmonics too, a tuba-ish sound); tune and chords are near-pure
//! tones. The tune is original to this project and dedicated to the public
//! domain (CC0), like the rest of the test pattern's content.
//!
//! The music peaks at [`MUSIC_PEAK`]: what's left of [`SIGNAL_PEAK`] after
//! the code's four tones. It sounds about as loud as the tones.

use std::f64::consts::TAU;

use super::code::{BANDS, TONE_AMPLITUDE, ToneCode};
use crate::FrameRate;

/// The highest the code and music together can reach, leaving 1 dB of
/// headroom for lossy codecs' overshoot.
pub const SIGNAL_PEAK: f64 = 0.9;
/// The music's peak amplitude: the room the code's four tones leave.
pub const MUSIC_PEAK: f64 = SIGNAL_PEAK - BANDS as f64 * TONE_AMPLITUDE;
/// Harmonics left empty between the music and the lowest code tone. (The
/// music is orthogonal to the code whatever the gap; the gap keeps it apart
/// in windows that don't line up with a frame, too.)
pub const GUARD: u32 = 1;
const BEATS_PER_SECOND: u32 = 2;
const LOOP_BEATS: u32 = 16;
/// "Do" is harmonic 8 in the score; transposed, it's this close to 240 Hz.
const DO_HZ: f64 = 240.0;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Voice {
    Tune,
    Chord,
    Bass,
}

impl Voice {
    /// Level, overtone amplitudes (from the fundamental up), decay time in
    /// seconds, and frames of attack.
    fn sound(self) -> (f64, &'static [f64], f64, u64) {
        match self {
            Voice::Tune => (1.0, &[1.0, 0.2], 0.8, 1),
            Voice::Chord => (0.4, &[1.0], 0.15, 0),
            Voice::Bass => (0.8, &[1.0, 0.55, 0.3, 0.18, 0.1], 0.4, 1),
        }
    }
}

use Voice::{Bass, Chord, Tune};

/// The score: beat, length in beats, voice, harmonics with "do" = 8.
/// Bar 1 (I) bounces and points; bar 2 (V) twirls; bar 3 (I7) flicks the
/// hat (the run climbs while it's in the air, landing on top with it); bar 4
/// (V) robots in staccato, runs up through the jump and lands on I.
#[rustfmt::skip]
const SCORE: &[(f64, f64, Voice, &[u32])] = &[
    (0.0, 0.5, Tune, &[10]), (0.5, 0.5, Tune, &[12]), (1.0, 1.0, Tune, &[16]),
    (2.0, 0.5, Tune, &[10]), (2.5, 0.5, Tune, &[12]), (3.0, 1.0, Tune, &[16]),
    (4.0, 0.5, Tune, &[15]), (4.5, 0.5, Tune, &[16]), (5.0, 0.5, Tune, &[15]), (5.5, 0.5, Tune, &[12]),
    (6.0, 0.5, Tune, &[9]), (6.5, 0.5, Tune, &[12]), (7.0, 1.0, Tune, &[15]),
    (8.0, 0.5, Tune, &[16]), (8.5, 0.5, Tune, &[12]),
    (9.0, 0.25, Tune, &[10]), (9.25, 0.25, Tune, &[12]), (9.5, 0.25, Tune, &[14]), (9.75, 0.25, Tune, &[15]),
    (10.0, 0.5, Tune, &[16]), (10.5, 0.5, Tune, &[14]), (11.0, 0.5, Tune, &[12]), (11.5, 0.5, Tune, &[10]),
    (12.0, 0.25, Tune, &[12]), (12.5, 0.25, Tune, &[12]), (13.0, 0.25, Tune, &[15]), (13.5, 0.25, Tune, &[15]),
    (14.0, 0.25, Tune, &[9]), (14.25, 0.25, Tune, &[12]), (14.5, 0.5, Tune, &[15]), (15.0, 1.0, Tune, &[16]),

    (1.0, 0.5, Chord, &[8, 10, 12]), (3.0, 0.5, Chord, &[8, 10, 12]),
    (5.0, 0.5, Chord, &[9, 12, 15]), (7.0, 0.5, Chord, &[9, 12, 15]),
    (9.0, 0.5, Chord, &[8, 10, 14]), (11.0, 0.5, Chord, &[8, 10, 14]),
    (13.0, 0.5, Chord, &[9, 12, 15]), (15.0, 1.0, Chord, &[8, 10, 12]),

    (0.0, 1.0, Bass, &[4]), (2.0, 1.0, Bass, &[3]),
    (4.0, 1.0, Bass, &[3]), (6.0, 1.0, Bass, &[6]),
    (8.0, 1.0, Bass, &[4]), (10.0, 1.0, Bass, &[3]),
    (12.0, 1.0, Bass, &[3]), (14.0, 0.5, Bass, &[3]), (15.0, 1.0, Bass, &[4]),
];

/// How the score's harmonics (do = 8) map onto the frame rate's.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Key {
    /// Every harmonic times this: for frame rates of 30 and below.
    Times(u32),
    /// Do on harmonic 4 (for about 48 to 60 fps), as a bugle would play it:
    /// re becomes sol and ti the natural seventh.
    Bugle,
}

impl Key {
    fn map(self, h: u32) -> u32 {
        match self {
            Key::Times(n) => h * n,
            Key::Bugle => match h {
                3 => 3,
                9 => 6,
                15 => 7,
                h => h.div_ceil(2),
            },
        }
    }

    fn do_harmonic(self) -> f64 {
        f64::from(self.map(8))
    }
}

/// The tune for one frame rate: for each frame of the 8-second loop, the
/// harmonics sounding and their amplitudes.
#[derive(Clone, Debug)]
pub struct Music {
    fps: FrameRate,
    frames: Vec<Vec<(f64, f64)>>,
}

impl Music {
    /// The tune for `code`'s frame rate, or `None` if the code leaves too
    /// few harmonics below it (above about 60 fps).
    pub fn new(code: &ToneCode) -> Option<Music> {
        let fps = code.fps();
        let highest = code.base_harmonic().checked_sub(GUARD + 1)?;
        let top = SCORE.iter().flat_map(|n| n.3).copied().max().unwrap_or(0);
        let key = [Key::Times(4), Key::Times(2), Key::Times(1), Key::Bugle]
            .into_iter()
            .filter(|k| k.map(top) <= highest)
            .min_by(|a, b| {
                let off = |k: &Key| (k.do_harmonic() * fps.as_f64() / DO_HZ).log2().abs();
                off(a).total_cmp(&off(b))
            })?;

        let nominal = u64::from(fps.nominal());
        let frames_per_beat = nominal as f64 / f64::from(BEATS_PER_SECOND);
        let loop_frames = nominal * u64::from(LOOP_BEATS / BEATS_PER_SECOND);
        let at_beat = |beat: f64| (beat * frames_per_beat).round() as u64;
        let mut table: Vec<Vec<(u32, f64)>> = vec![Vec::new(); loop_frames as usize];
        let release = 2;
        for &(beat, beats, voice, harmonics) in SCORE {
            let (level, overtones, decay, attack) = voice.sound();
            let (start, end) = (at_beat(beat), at_beat(beat + beats));
            let mut notes: Vec<u32> = harmonics.iter().map(|&h| key.map(h)).collect();
            notes.dedup();
            for f in start..end + release {
                let i = f - start;
                let mut env = level * (-(i as f64) / nominal as f64 / decay).exp();
                if i < attack {
                    env *= (i + 1) as f64 / (attack + 1) as f64;
                }
                if f >= end {
                    env *= 1.0 - (f - end + 1) as f64 / (release + 1) as f64;
                }
                let frame = &mut table[(f % loop_frames) as usize];
                for &h in &notes {
                    for (m, &a) in (1..).zip(overtones) {
                        if h * m <= highest {
                            frame.push((h * m, env * a));
                        }
                    }
                }
            }
        }
        // Merge each frame's partials and scale the loudest frame to the peak.
        let frames: Vec<Vec<(u32, f64)>> = table
            .into_iter()
            .map(|mut partials| {
                partials.sort_by_key(|p| p.0);
                let mut merged: Vec<(u32, f64)> = Vec::new();
                for (h, a) in partials {
                    match merged.last_mut() {
                        Some(last) if last.0 == h => last.1 += a,
                        _ => merged.push((h, a)),
                    }
                }
                merged
            })
            .collect();
        let loudest = frames
            .iter()
            .map(|f| f.iter().map(|p| p.1.abs()).sum::<f64>())
            .fold(0.0, f64::max);
        let scale = if loudest > 0.0 {
            MUSIC_PEAK / loudest
        } else {
            0.0
        };
        Some(Music {
            fps,
            frames: frames
                .into_iter()
                .map(|f| {
                    f.into_iter()
                        .map(|(h, a)| (f64::from(h), a * scale))
                        .collect()
                })
                .collect(),
        })
    }

    /// The highest harmonic of the frame rate the music uses.
    pub fn highest_harmonic(&self) -> u32 {
        self.frames
            .iter()
            .flatten()
            .map(|p| p.0 as u32)
            .max()
            .unwrap_or(0)
    }

    /// The music at source time `t` seconds (silence before zero).
    pub fn signal_at(&self, t: f64) -> f64 {
        if t < 0.0 {
            return 0.0;
        }
        let position = t * self.fps.as_f64();
        let frame = position.floor();
        let frac = position - frame;
        let n = self.frames.len() as u64;
        self.frames[(frame as u64 % n) as usize]
            .iter()
            .map(|&(k, a)| a * (TAU * k * frac).sin())
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::detect::CellDetector;
    use crate::audio::synth::ToneSynth;

    fn code(fps: &str) -> ToneCode {
        ToneCode::new(fps.parse().unwrap(), 48000).unwrap()
    }

    #[test]
    fn stays_below_the_code_and_within_full_scale() {
        for fps in [
            "15", "23.976", "24", "25", "29.97", "30", "48", "50", "59.94", "60",
        ] {
            let code = code(fps);
            let music = Music::new(&code).unwrap_or_else(|| panic!("{fps}: no music"));
            assert!(
                music.highest_harmonic() + GUARD < code.base_harmonic(),
                "{fps}"
            );
            let peak = (0..96_000)
                .map(|i| music.signal_at(i as f64 / 48_000.0).abs())
                .fold(0.0, f64::max);
            assert!(
                peak <= MUSIC_PEAK + 1e-9 && peak > 0.5 * MUSIC_PEAK,
                "{fps}: peak {peak}"
            );
        }
        for fps in ["100", "120"] {
            assert!(Music::new(&code(fps)).is_none(), "{fps}");
        }
    }

    #[test]
    fn is_silent_at_frame_boundaries_and_loops() {
        let code = code("29.97");
        let music = Music::new(&code).unwrap();
        for frame in [1i64, 15, 239, 240, 241, 1000] {
            let t = code.fps().frame_time(frame);
            assert!(music.signal_at(t + 1e-9).abs() < 1e-4, "frame {frame}");
        }
        let loop_secs = 240.0 / code.fps().as_f64();
        for t in [0.01, 1.234, 5.5] {
            assert!((music.signal_at(t) - music.signal_at(t + loop_secs)).abs() < 1e-6);
        }
        assert_eq!(music.signal_at(-1.0), 0.0);
    }

    #[test]
    fn the_decoder_does_not_hear_it() {
        // Decoding the code with and without the music gives the same cells,
        // to well within a microsecond.
        for fps in ["24", "29.97", "30", "50", "60"] {
            let code = code(fps);
            let count = 3 * 48_000;
            let render = |music: bool| {
                let mut synth = ToneSynth::new(code.clone(), 0.0123);
                if music {
                    synth = synth.with_music();
                }
                let mut out = vec![0.0; count];
                synth.fill(&mut out);
                out
            };
            let detector = CellDetector::new(code.clone());
            let plain = detector.detect(&render(false), 0.0);
            let mixed = detector.detect(&render(true), 0.0);
            assert!(plain.len() > 60, "{fps}: {} cells", plain.len());
            assert_eq!(plain.len(), mixed.len(), "{fps}");
            for (a, b) in plain.iter().zip(&mixed) {
                assert_eq!((a.id, a.sample), (b.id, b.sample), "{fps}");
                let shift = (a.residual - b.residual).abs();
                assert!(
                    shift < 1e-6,
                    "{fps}: frame {} moved {:.3} µs",
                    a.id,
                    shift * 1e6
                );
            }
        }
    }
}
