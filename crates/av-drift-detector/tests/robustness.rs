//! Captures made in poor conditions, simulated in memory (no FFmpeg): digital
//! copies rotated, skewed, blurred or shrunk; cameras whose exposures blend two
//! frames, with rolling shutters; and audio that's quiet, notched or echoey.
//! Each is analysed end to end and checked against the known answer.

use std::collections::HashMap;
use std::f64::consts::TAU;

use av_drift_detector::analyze::{AnalyzeOptions, analyze_scan};
use av_drift_detector::audio::code::TONE_AMPLITUDE;
use av_drift_detector::audio::{ToneCode, synthesize};
use av_drift_detector::media::{AudioTrack, MediaScan, VideoSample};
use av_drift_detector::pattern::canvas::{LumaImage, LumaView};
use av_drift_detector::pattern::{PatternRenderer, StampReader, StampSource};
use av_drift_detector::{CaptureMode, DriftReport, FrameRate};

const RATE: u32 = 48_000;

/// Test-pattern frames, rendered once each.
struct Pattern {
    renderer: PatternRenderer,
    fps: FrameRate,
    frames: HashMap<u32, LumaImage>,
}

impl Pattern {
    fn new(fps: &str, width: usize, height: usize) -> Self {
        let fps: FrameRate = fps.parse().unwrap();
        let tones = ToneCode::new(fps, RATE).unwrap();
        Pattern {
            renderer: PatternRenderer::new(width, height, tones).unwrap(),
            fps,
            frames: HashMap::new(),
        }
    }

    fn size(&self) -> (usize, usize) {
        let l = self.renderer.layout();
        (l.width, l.height)
    }

    fn frame(&mut self, id: u32) -> &LumaImage {
        let (w, h) = self.size();
        let renderer = &self.renderer;
        self.frames.entry(id).or_insert_with(|| {
            let mut img = LumaImage::new(w, h, 0);
            renderer.render(id, &mut img.canvas()).unwrap();
            img
        })
    }
}

/// `img` at `(x, y)`, interpolated; black outside.
fn bilinear(img: &LumaImage, x: f64, y: f64) -> f64 {
    let (w, h) = (img.width as f64, img.height as f64);
    if !(0.0..w - 1.0).contains(&x) || !(0.0..h - 1.0).contains(&y) {
        return 16.0;
    }
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let at = |dx: usize, dy: usize| {
        f64::from(img.data[(y0 as usize + dy) * img.width + x0 as usize + dx])
    };
    (at(0, 0) * (1.0 - fx) + at(1, 0) * fx) * (1.0 - fy)
        + (at(0, 1) * (1.0 - fx) + at(1, 1) * fx) * fy
}

/// A projective map taking each of `from` to the matching point of `to`.
fn homography(from: [(f64, f64); 4], to: [(f64, f64); 4]) -> impl Fn(f64, f64) -> (f64, f64) {
    let mut a = [[0.0f64; 9]; 8];
    for i in 0..4 {
        let ((u, v), (x, y)) = (from[i], to[i]);
        a[2 * i] = [u, v, 1.0, 0.0, 0.0, 0.0, -u * x, -v * x, x];
        a[2 * i + 1] = [0.0, 0.0, 0.0, u, v, 1.0, -u * y, -v * y, y];
    }
    for c in 0..8 {
        let p = (c..8)
            .max_by(|&i, &j| a[i][c].abs().total_cmp(&a[j][c].abs()))
            .unwrap();
        a.swap(c, p);
        let pivot = a[c];
        for (r, row) in a.iter_mut().enumerate() {
            if r != c {
                let f = row[c] / pivot[c];
                for (x, p) in row.iter_mut().zip(pivot).skip(c) {
                    *x -= f * p;
                }
            }
        }
    }
    let h: [f64; 8] = std::array::from_fn(|i| a[i][8] / a[i][i]);
    move |u, v| {
        let d = h[6] * u + h[7] * v + 1.0;
        (
            (h[0] * u + h[1] * v + h[2]) / d,
            (h[3] * u + h[4] * v + h[5]) / d,
        )
    }
}

/// How a picture's pixels map back to the test frame's.
type Lens = Box<dyn Fn(f64, f64) -> (f64, f64)>;

/// Something done to a test frame on its way to a capture.
type Damage = Box<dyn Fn(&LumaImage) -> LumaImage>;

/// Rotation by `degrees` about the middle, shrunk by `scale` so the corners stay in.
fn rotation(w: usize, h: usize, degrees: f64, scale: f64) -> Lens {
    let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
    let (s, c) = (-degrees.to_radians()).sin_cos();
    Box::new(move |x, y| {
        let (dx, dy) = ((x - cx) / scale, (y - cy) / scale);
        (cx + c * dx - s * dy, cy + s * dx + c * dy)
    })
}

/// A keystoned view: the test frame's corners land at `corners` in the picture.
fn perspective(w: usize, h: usize, corners: [(f64, f64); 4]) -> Lens {
    let (w, h) = (w as f64, h as f64);
    Box::new(homography(
        corners,
        [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)],
    ))
}

fn warp(src: &LumaImage, out: (usize, usize), lens: &dyn Fn(f64, f64) -> (f64, f64)) -> LumaImage {
    let mut img = LumaImage::new(out.0, out.1, 16);
    for y in 0..out.1 {
        for x in 0..out.0 {
            let (sx, sy) = lens(x as f64, y as f64);
            img.data[y * out.0 + x] = bilinear(src, sx, sy).round() as u8;
        }
    }
    img
}

/// Gaussian blur, separably.
fn blur(img: &LumaImage, sigma: f64) -> LumaImage {
    let r = (3.0 * sigma).ceil() as i64;
    let kernel: Vec<f64> = (-r..=r)
        .map(|i| (-(i * i) as f64 / (2.0 * sigma * sigma)).exp())
        .collect();
    let total: f64 = kernel.iter().sum();
    let (w, h) = (img.width as i64, img.height as i64);
    let pass = |src: &[u8], horizontal: bool| -> Vec<u8> {
        let mut out = vec![0u8; src.len()];
        for y in 0..h {
            for x in 0..w {
                let mut sum = 0.0;
                for (k, &g) in kernel.iter().enumerate() {
                    let d = k as i64 - r;
                    let (sx, sy) = if horizontal {
                        ((x + d).clamp(0, w - 1), y)
                    } else {
                        (x, (y + d).clamp(0, h - 1))
                    };
                    sum += g * f64::from(src[(sy * w + sx) as usize]);
                }
                out[(y * w + x) as usize] = (sum / total).round() as u8;
            }
        }
        out
    };
    let mut out = img.clone();
    out.data = pass(&pass(&img.data, true), false);
    out
}

/// Deterministic noise of about ±`amplitude` levels.
fn add_noise(img: &mut LumaImage, amplitude: f64, seed: u32) {
    let mut s = seed;
    for p in img.data.iter_mut() {
        s = s.wrapping_mul(1_103_515_245).wrapping_add(12345);
        let n = (f64::from(s >> 8) / f64::from(1u32 << 24) - 0.5) * 2.0 * amplitude;
        *p = (f64::from(*p) + n).round().clamp(0.0, 255.0) as u8;
    }
}

/// Glare: the picture washed out more and more towards the right.
fn glare(img: &mut LumaImage) {
    let w = img.width as f64;
    for (i, p) in img.data.iter_mut().enumerate() {
        let x = (i % img.width) as f64;
        *p = (f64::from(*p) * 0.55 + 110.0 * x / w)
            .round()
            .clamp(0.0, 255.0) as u8;
    }
}

/// The test audio, `delay` seconds late, `seconds` long.
fn audio(fps: FrameRate, delay: f64, seconds: f64) -> AudioTrack {
    let code = ToneCode::new(fps, RATE).unwrap();
    AudioTrack {
        sample_rate: RATE,
        start_time: 0.0,
        samples: synthesize(&code, delay, 0, (seconds * f64::from(RATE)) as usize),
    }
}

fn analyse(video: Vec<VideoSample>, audio: AudioTrack, size: (usize, usize)) -> DriftReport {
    let scan = MediaScan {
        video,
        audio: Some(audio),
        width: size.0 as u32,
        height: size.1 as u32,
        stream_fps: None,
    };
    analyze_scan(&scan, &AnalyzeOptions::default(), "simulated".into()).unwrap()
}

/// Reads every picture with one reader, as a decoder would.
fn read_all(pictures: impl IntoIterator<Item = (f64, LumaImage)>) -> Vec<VideoSample> {
    let mut reader = StampReader::new(true);
    let mut video: Vec<VideoSample> = pictures
        .into_iter()
        .map(|(t, img)| reader.read(&img.view(), t))
        .collect();
    reader.finish(&mut video);
    video
}

fn mean_ms(report: &DriftReport) -> f64 {
    report.summary.mean_ms.expect("measured")
}

// ------------------------------------------------------------ digital copies --

#[test]
fn digital_copies_read_through_geometric_and_optical_damage() {
    let mut pattern = Pattern::new("30", 1280, 720);
    let (w, h) = pattern.size();
    let keystone = [(60.0, 40.0), (1250.0, 10.0), (1220.0, 690.0), (30.0, 715.0)];
    let damage: Vec<(&str, Damage)> = vec![
        (
            "rotated 10°",
            Box::new(move |f| warp(f, (w, h), &rotation(w, h, 10.0, 0.85))),
        ),
        (
            "rotated 30°",
            Box::new(move |f| warp(f, (w, h), &rotation(w, h, 30.0, 0.65))),
        ),
        (
            "rotated 90°",
            Box::new(move |f| warp(f, (w, h), &rotation(w, h, 90.0, 0.55))),
        ),
        (
            "keystoned",
            Box::new(move |f| warp(f, (w, h), &perspective(w, h, keystone))),
        ),
        (
            "keystoned and blurred",
            Box::new(move |f| blur(&warp(f, (w, h), &perspective(w, h, keystone)), 2.0)),
        ),
        (
            "glare",
            Box::new(move |f| {
                let mut img = warp(f, (w, h), &rotation(w, h, 5.0, 0.9));
                glare(&mut img);
                img
            }),
        ),
        (
            "noisy",
            Box::new(move |f| {
                let mut img = warp(f, (w, h), &rotation(w, h, 8.0, 0.9));
                add_noise(&mut img, 30.0, 7);
                img
            }),
        ),
    ];
    let fps = pattern.fps;
    for (name, spoil) in &damage {
        // One second of frames, each timestamped when it starts, as a copy is.
        let pictures: Vec<(f64, LumaImage)> = (0..30)
            .map(|n| (fps.frame_time(n), spoil(pattern.frame(n as u32))))
            .collect();
        let video = read_all(pictures);
        let read = video.iter().filter(|v| v.stamp.is_some()).count();
        assert!(read >= 28, "{name}: {read} of 30 read");
        for (n, v) in video.iter().enumerate() {
            if let Some(s) = v.stamp {
                assert_eq!(s.frame_id, n as u32, "{name}");
            }
        }
        let report = analyse(video, audio(fps, 0.040, 1.6), (w, h));
        // A still picture with frames at their own timestamps: a copy, however mangled.
        assert_eq!(
            report.capture,
            CaptureMode::File,
            "{name}\n{}",
            report.summary_text()
        );
        assert!(
            (mean_ms(&report) - 40.0).abs() < 0.1,
            "{name}\n{}",
            report.summary_text()
        );
    }
}

#[test]
fn a_small_code_in_a_big_frame_reads() {
    // A 1100-pixel-wide test frame shown inside a 4K picture.
    let mut pattern = Pattern::new("25", 1280, 720);
    let (w, h) = pattern.size();
    let scale = 1100.0 / w as f64;
    let (ox, oy) = (2400.0, 900.0);
    let lens = move |x: f64, y: f64| ((x - ox) / scale, (y - oy) / scale);
    let pictures: Vec<(f64, LumaImage)> = (0..25)
        .map(|n| (n as f64 / 25.0, warp(pattern.frame(n), (3840, 2160), &lens)))
        .collect();
    let video = read_all(pictures);
    let read = video
        .iter()
        .filter(|v| v.stamp.is_some_and(|s| s.frame_id < 25))
        .count();
    assert!(read >= 24, "{read} of 25 read");
    let _ = h;
}

// ------------------------------------------------------------------- cameras --

/// A camera filming the display: the display shows frame N from `start + N/fps`;
/// the camera takes a picture every `1/camera_fps` from `first`, each exposed
/// for `exposure` from its timestamp, a rolling shutter starting each row of the
/// picture `readout · row / height` later. Pictures that span a switch blend
/// the two frames, row by row.
struct Camera {
    fps: f64,
    camera_fps: f64,
    exposure: f64,
    readout: f64,
    start: f64,
    first: f64,
    size: (usize, usize),
    lens: Lens,
    blur: f64,
    noise: f64,
}

impl Camera {
    fn film(&self, pattern: &mut Pattern, seconds: f64) -> Vec<(f64, LumaImage)> {
        let (w, h) = self.size;
        (0..(seconds * self.camera_fps) as usize)
            .map(|k| {
                let t = self.first + k as f64 / self.camera_fps;
                let mut img = LumaImage::new(w, h, 16);
                for y in 0..h {
                    let row = t + self.readout * y as f64 / h as f64;
                    let shown = ((row - self.start) * self.fps).floor();
                    if shown < 0.0 {
                        continue;
                    }
                    let switch = self.start + (shown + 1.0) / self.fps;
                    let after = ((row + self.exposure - switch) / self.exposure).clamp(0.0, 1.0);
                    let (a, b) = (shown as u32, shown as u32 + 1);
                    pattern.frame(a);
                    pattern.frame(b);
                    let (fa, fb) = (&pattern.frames[&a], &pattern.frames[&b]);
                    for x in 0..w {
                        let (sx, sy) = (self.lens)(x as f64, y as f64);
                        let v = bilinear(fa, sx, sy) * (1.0 - after) + bilinear(fb, sx, sy) * after;
                        img.data[y * w + x] = v.round() as u8;
                    }
                }
                if self.blur > 0.0 {
                    img = blur(&img, self.blur);
                }
                if self.noise > 0.0 {
                    add_noise(&mut img, self.noise, k as u32 + 1);
                }
                (t, img)
            })
            .collect()
    }

    /// The drift the capture should report for audio `delay` seconds late.
    fn truth_ms(&self, delay: f64) -> f64 {
        (delay - self.start) * 1000.0
    }
}

fn handheld(w: usize, h: usize) -> Lens {
    perspective(
        w,
        h,
        [(70.0, 50.0), (1230.0, 20.0), (1200.0, 700.0), (40.0, 690.0)],
    )
}

#[test]
fn a_camera_at_another_rate_is_timed_from_blends() {
    // 24 fps filming 25 fps, 1/50 s exposures, a 12 ms rolling shutter,
    // keystoned, a little soft and noisy.
    let mut pattern = Pattern::new("25", 1280, 720);
    let (w, h) = pattern.size();
    let camera = Camera {
        fps: 25.0,
        camera_fps: 24.0,
        exposure: 0.02,
        readout: 0.012,
        start: 0.2,
        first: 0.237,
        size: (w, h),
        lens: handheld(w, h),
        blur: 1.0,
        noise: 8.0,
    };
    let delay = 0.245;
    let video = read_all(camera.film(&mut pattern, 4.0));
    let report = analyse(video, audio(pattern.fps, delay, 4.8), (w, h));
    let text = report.summary_text();
    assert_eq!(report.capture, CaptureMode::Camera, "{text}");
    assert_eq!(report.video_timing, Some("blend"), "{text}");
    let exposure = report.exposure_ms.expect("exposure measured");
    assert!((exposure - 20.0).abs() < 1.5, "exposure {exposure}\n{text}");
    let error = mean_ms(&report) - camera.truth_ms(delay);
    assert!(error.abs() < 1.0, "off by {error:.2} ms\n{text}");
    assert!(report.video_timing_uncertainty_ms.unwrap() < 2.0, "{text}");
    assert!(report.summary.measured as f64 > 0.9 * 4.0 * 24.0, "{text}");
}

#[test]
fn a_camera_locked_to_the_pattern_reports_an_honest_uncertainty() {
    // 30 fps on 30 fps: every picture is caught at the same phase. Frame
    // numbers alone leave half a frame (±16.7 ms) open; the blends narrow it.
    let mut pattern = Pattern::new("30", 1280, 720);
    let (w, h) = pattern.size();
    for (first, exposure) in [(0.2 + 0.7 / 30.0, 0.025), (0.2 + 0.4 / 30.0, 0.03)] {
        let camera = Camera {
            fps: 30.0,
            camera_fps: 30.0,
            exposure,
            readout: 0.008,
            start: 0.2,
            first,
            size: (w, h),
            lens: handheld(w, h),
            blur: 0.8,
            noise: 6.0,
        };
        let delay = 0.23;
        let video = read_all(camera.film(&mut pattern, 3.0));
        let report = analyse(video, audio(pattern.fps, delay, 3.8), (w, h));
        let text = report.summary_text();
        assert_eq!(report.capture, CaptureMode::Camera, "{text}");
        assert_eq!(report.video_timing, Some("blend"), "{text}");
        let uncertainty = report.video_timing_uncertainty_ms.unwrap();
        assert!(uncertainty < 12.0, "±{uncertainty}\n{text}");
        let error = mean_ms(&report) - camera.truth_ms(delay);
        assert!(
            error.abs() <= uncertainty + 0.5,
            "off by {error:.2} ms ± {uncertainty:.2}\n{text}"
        );
    }
}

#[test]
fn a_screen_recording_at_another_rate_is_timed_as_sampled() {
    // A pixel-perfect recording at 25 fps of a 30 fps video: each picture is
    // whatever frame was on screen, so frames come at every phase.
    let mut pattern = Pattern::new("30", 640, 360);
    let (w, h) = pattern.size();
    let pictures: Vec<(f64, LumaImage)> = (0..75)
        .map(|k| {
            let t = 0.011 + k as f64 / 25.0;
            (t, pattern.frame((t * 30.0).floor() as u32).clone())
        })
        .collect();
    let report = analyse(read_all(pictures), audio(pattern.fps, 0.05, 3.6), (w, h));
    let text = report.summary_text();
    assert_eq!(report.capture, CaptureMode::Camera, "{text}");
    let uncertainty = report.video_timing_uncertainty_ms.unwrap();
    assert!(
        (mean_ms(&report) - 50.0).abs() <= uncertainty + 0.2,
        "{text}"
    );
    assert!(uncertainty < 4.0, "{text}");
}

// --------------------------------------------------------------------- audio --

/// One second of clean video stamps, as a copy has.
fn copy_frames(pattern: &mut Pattern, seconds: f64) -> Vec<VideoSample> {
    let fps = pattern.fps;
    let n = (seconds * fps.as_f64()) as u32;
    read_all((0..n).map(|i| (fps.frame_time(i64::from(i)), pattern.frame(i).clone())))
}

/// The test audio built from some bands only, at `gain`, `delay` seconds late,
/// with noise at `noise` (amplitude) and `taps` echoes of (delay s, gain).
fn shaped_audio(
    fps: FrameRate,
    delay: f64,
    seconds: f64,
    bands: &[usize],
    gain: f64,
    noise: f64,
    taps: &[(f64, f64)],
) -> AudioTrack {
    let code = ToneCode::new(fps, RATE).unwrap();
    let n = (seconds * f64::from(RATE)) as usize;
    let dry: Vec<f64> = (0..n)
        .map(|i| {
            let t = i as f64 / f64::from(RATE) - delay;
            bands
                .iter()
                .filter_map(|&b| code.band_phase(b, t))
                .map(|phase| TONE_AMPLITUDE * phase.sin())
                .sum::<f64>()
                * gain
        })
        .collect();
    let mut wet = dry.clone();
    for &(d, g) in taps {
        let lag = (d * f64::from(RATE)) as usize;
        for i in lag..n {
            wet[i] += g * dry[i - lag];
        }
    }
    let mut seed = 77u32;
    let samples = wet
        .iter()
        .map(|&v| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            (v + noise * (f64::from(seed >> 8) / f64::from(1u32 << 24) - 0.5) * 2.0) as f32
        })
        .collect();
    AudioTrack {
        sample_rate: RATE,
        start_time: 0.0,
        samples,
    }
}

#[test]
fn quiet_recordings_decode_above_their_own_noise() {
    let mut pattern = Pattern::new("30", 640, 360);
    let video = copy_frames(&mut pattern, 2.0);
    // −50 and −70 dB below the generated level, over a noise floor 20 dB
    // below the quietest tones.
    for gain_db in [-50.0, -70.0] {
        let gain = 10f64.powf(gain_db / 20.0);
        let track = shaped_audio(
            pattern.fps,
            0.03,
            2.6,
            &[0, 1, 2, 3],
            gain,
            0.1 * gain * TONE_AMPLITUDE,
            &[],
        );
        let report = analyse(video.clone(), track, pattern.size());
        let text = report.summary_text();
        assert!(report.summary.measured >= 55, "{gain_db} dB\n{text}");
        assert!(
            (mean_ms(&report) - 30.0).abs() < 0.05,
            "{gain_db} dB\n{text}"
        );
    }
}

#[test]
fn a_band_lost_altogether_is_filled_in() {
    // No band 3 at all (a low-pass speaker at a high frame rate, say): its
    // nibble comes from the video.
    let mut pattern = Pattern::new("30", 640, 360);
    let video = copy_frames(&mut pattern, 2.0);
    let track = shaped_audio(pattern.fps, 0.03, 2.6, &[0, 1, 2], 1.0, 1e-4, &[]);
    let report = analyse(video, track, pattern.size());
    let text = report.summary_text();
    assert!(report.summary.measured >= 55, "{text}");
    assert!((mean_ms(&report) - 30.0).abs() < 0.05, "{text}");
}

#[test]
fn a_notch_at_one_tone_costs_little() {
    // A deep notch at one band-2 tone. The tones hop, so it takes out band 2
    // in one frame of every sixteen, and the vote fills it in.
    let mut pattern = Pattern::new("30", 640, 360);
    let video = copy_frames(&mut pattern, 2.0);
    let mut track = shaped_audio(pattern.fps, 0.03, 2.6, &[0, 1, 2, 3], 1.0, 1e-4, &[]);
    let code = ToneCode::new(pattern.fps, RATE).unwrap();
    let f = code.frequency(2, 5);
    // Biquad notch, twice over.
    let w0 = TAU * f / f64::from(RATE);
    let alpha = w0.sin() / (2.0 * 30.0);
    let (b0, b1, b2) = (1.0, -2.0 * w0.cos(), 1.0);
    let (a0, a1, a2) = (1.0 + alpha, -2.0 * w0.cos(), 1.0 - alpha);
    for _ in 0..2 {
        let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
        for s in track.samples.iter_mut() {
            let x = f64::from(*s);
            let y = (b0 * x + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2) / a0;
            (x2, x1, y2, y1) = (x1, x, y1, y);
            *s = y as f32;
        }
    }
    let report = analyse(video, track, pattern.size());
    let text = report.summary_text();
    assert!(report.summary.measured >= 55, "{text}");
    // The notch's own phase response shifts nearby tones a little.
    assert!((mean_ms(&report) - 30.0).abs() < 0.3, "{text}");
}

#[test]
fn strong_echo_keeps_the_timing() {
    // Reflections at 23, 61 and 127 ms of 60%, 45% and 30%: a harsh room.
    let mut pattern = Pattern::new("30", 640, 360);
    let video = copy_frames(&mut pattern, 3.0);
    let taps = [(0.023, 0.6), (0.061, 0.45), (0.127, 0.3)];
    let track = shaped_audio(pattern.fps, 0.03, 3.6, &[0, 1, 2, 3], 0.5, 1e-4, &taps);
    let report = analyse(video, track, pattern.size());
    let text = report.summary_text();
    assert!(report.summary.measured >= 45, "{text}");
    assert!((mean_ms(&report) - 30.0).abs() < 0.3, "{text}");
    assert!(report.summary.skew_ppm.unwrap().abs() < 5.0, "{text}");
}

#[test]
fn old_format_files_get_a_clear_error() {
    // A frame whose barcode is version 1 (0.3 and earlier) and no QR code.
    let mut img = LumaImage::new(640, 360, 16);
    let word = 1u64 << 44 | 5u64 << 40 | 1234u64 << 16;
    let crc = {
        let mut c = 0u8;
        for &b in &word.to_be_bytes()[2..7] {
            c ^= b;
            for _ in 0..8 {
                c = if c & 0x80 != 0 {
                    (c << 1) ^ 0x07
                } else {
                    c << 1
                };
            }
        }
        c ^ 0x55
    };
    av_drift_detector::pattern::barcode::draw_guards(&mut img.canvas());
    av_drift_detector::pattern::barcode::draw(&mut img.canvas(), word | u64::from(crc));
    let mut reader = StampReader::new(true);
    let sample = reader.read(&LumaView::new(&img.data, 640, 360, 640), 0.0);
    assert!(sample.stamp.is_none() && sample.old_format);
    let scan = MediaScan {
        video: vec![sample],
        audio: None,
        width: 640,
        height: 360,
        stream_fps: None,
    };
    let err = analyze_scan(&scan, &AnalyzeOptions::default(), "old".into()).unwrap_err();
    assert!(err.to_string().contains("older format"), "{err}");
}

#[test]
fn stamps_say_how_they_were_read() {
    // Clean frames read from the barcode; with it wiped, from the QR code;
    // a frame caught mid-switch, by fitting.
    let mut pattern = Pattern::new("25", 640, 360);
    let mut reader = StampReader::new(true);
    let clean = reader.read(&pattern.frame(10).view(), 0.4);
    assert_eq!(clean.stamp.unwrap().source, StampSource::Barcode);
    let wipe = |img: &LumaImage| {
        let mut img = img.clone();
        img.data[..640 * 100].fill(16);
        img
    };
    let qr = reader.read(&wipe(pattern.frame(11)).view(), 0.44);
    assert_eq!(qr.stamp.unwrap().source, StampSource::Qr);
    let (a, b) = (wipe(pattern.frame(12)), wipe(pattern.frame(13)));
    let mut mixed = a.clone();
    for (m, (&x, &y)) in mixed.data.iter_mut().zip(a.data.iter().zip(&b.data)) {
        *m = ((u16::from(x) + u16::from(y)) / 2) as u8;
    }
    let blended = reader.read(&mixed.view(), 0.48).stamp.unwrap();
    assert!(blended.source == StampSource::QrBlend || blended.source == StampSource::Qr);
    let blend = blended.blend.unwrap();
    assert_eq!(blend.first, 12);
    assert!((blend.weight_next - 0.5).abs() < 0.03, "{blend:?}");
}
