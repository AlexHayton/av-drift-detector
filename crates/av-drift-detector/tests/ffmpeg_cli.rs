//! The `ffmpeg-cli` backend (running the `ffmpeg` and `ffprobe` programs)
//! against the linked one: files either one writes must measure the same,
//! frame for frame, through either.
//!
//! `cargo test -p av-drift-detector --features ffmpeg-cli --test ffmpeg_cli`
#![cfg(all(feature = "ffmpeg", feature = "ffmpeg-cli"))]

use std::path::{Path, PathBuf};
use std::process::Command;

use av_drift_detector::media::ffmpeg_cli;
use av_drift_detector::{
    AnalyzeOptions, AudioCodec, CaptureMode, DriftReport, FrameRate, GenerateOptions, VideoCodec,
    analyze, generate,
};

fn have_programs() -> bool {
    let ok = |name: &str| {
        Command::new(name)
            .arg("-version")
            .output()
            .is_ok_and(|o| o.status.success())
    };
    let found = ok("ffmpeg") && ok("ffprobe");
    if !found {
        eprintln!("skipping: ffmpeg and ffprobe aren't on PATH");
    }
    found
}

fn quiet() {
    ffmpeg_next::log::set_level(ffmpeg_next::log::Level::Error);
}

fn options(path: &Path, fps: &str, seconds: f64, offset_ms: f64) -> GenerateOptions {
    let fps: FrameRate = fps.parse().unwrap();
    let mut opts = GenerateOptions::new(path, fps, seconds);
    opts.width = 640;
    opts.height = 360;
    opts.audio_delay = offset_ms / 1000.0;
    opts
}

/// Every frame the same, and the drift the same to a nanosecond.
fn assert_same(a: &DriftReport, b: &DriftReport, what: &str) {
    assert_eq!((a.fps, a.capture), (b.fps, b.capture), "{what}");
    assert_eq!(a.frames.len(), b.frames.len(), "{what}: frame counts");
    for (x, y) in a.frames.iter().zip(&b.frames) {
        assert!(
            (x.time - y.time).abs() < 1e-9,
            "{what}: {} vs {}",
            x.time,
            y.time
        );
        assert_eq!(x.video_frame, y.video_frame, "{what} at {}", x.time);
        match (x.drift_ms, y.drift_ms) {
            (Some(p), Some(q)) => assert!((p - q).abs() < 1e-6, "{what} at {}: {p} vs {q}", x.time),
            (p, q) => assert_eq!(p, q, "{what} at {}", x.time),
        }
    }
    assert_eq!(a.summary.measured, b.summary.measured, "{what}");
}

fn assert_drift(report: &DriftReport, offset_ms: f64, tolerance: f64, what: &str) {
    let s = &report.summary;
    assert!(s.measured > 0, "{what}: {}", report.summary_text());
    for d in report.frames.iter().filter_map(|f| f.drift_ms) {
        assert!(
            (d - offset_ms).abs() < tolerance,
            "{what}: {d} vs {offset_ms}"
        );
    }
}

#[test]
fn files_written_by_the_programs_measure_true() {
    if !have_programs() {
        return;
    }
    quiet();
    let dir = tempfile::tempdir().unwrap();
    let cases = [
        (
            "h264.mp4",
            "29.97",
            25.0,
            VideoCodec::H264,
            AudioCodec::Aac,
            0.5,
        ),
        (
            "ffv1.mov",
            "30",
            -12.3,
            VideoCodec::Ffv1,
            AudioCodec::Pcm,
            0.05,
        ),
        (
            "h264.mkv",
            "25",
            78.5,
            VideoCodec::H264,
            AudioCodec::Aac,
            1.0,
        ),
    ];
    for (name, fps, offset, video, audio, tolerance) in cases {
        let path = dir.path().join(name);
        let mut opts = options(&path, fps, 3.0, offset);
        (opts.video_codec, opts.audio_codec) = (video, audio);
        let mut progress = 0;
        ffmpeg_cli::generate(&opts, |p| progress = p.frames_done).unwrap();
        assert_eq!(progress, opts.total_frames(), "{name}");

        let linked = analyze(&path, &AnalyzeOptions::default()).unwrap();
        assert_eq!(linked.capture, CaptureMode::File, "{name}");
        assert_eq!(linked.frames.len() as u64, opts.total_frames(), "{name}");
        assert_drift(&linked, offset, tolerance, name);
        let programs = ffmpeg_cli::analyze(&path, &AnalyzeOptions::default()).unwrap();
        assert_same(&linked, &programs, name);
    }
}

#[test]
fn the_programs_read_files_exactly_as_the_linked_decoder_does() {
    if !have_programs() {
        return;
    }
    quiet();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("linked.mp4");
    generate(&options(&path, "30", 6.0, 40.0), |_| {}).unwrap();

    for (offset, duration) in [(0.0, None), (1.0, Some(1.0)), (3.5, Some(2.0)), (4.0, None)] {
        let opts = AnalyzeOptions {
            offset,
            duration,
            ..AnalyzeOptions::default()
        };
        let linked = analyze(&path, &opts).unwrap();
        let programs = ffmpeg_cli::analyze(&path, &opts).unwrap();
        assert_same(
            &linked,
            &programs,
            &format!("window {offset} + {duration:?}"),
        );
        assert_drift(&programs, 40.0, 0.5, "programs");
    }
}

/// The filmed-screen simulation from tests/roundtrip.rs: only the QR code reads.
#[test]
fn a_filmed_screen_reads_the_same() {
    if !have_programs() {
        return;
    }
    quiet();
    let dir = tempfile::tempdir().unwrap();
    let screen = dir.path().join("screen.mp4");
    let mut opts = options(&screen, "30", 6.0, 40.0);
    (opts.width, opts.height) = (1280, 720);
    generate(&opts, |_| {}).unwrap();
    let phone: PathBuf = dir.path().join("phone.mp4");
    let status = Command::new("ffmpeg")
        .args(["-loglevel", "error", "-y", "-i"])
        .arg(&screen)
        .args([
            "-vf",
            "pad=1600:1000:160:140:color=0x303030,rotate=0.06:fillcolor=0x303030,gblur=sigma=1.2,\
             scale=1920:1080,fps=24.93:round=up",
            "-af",
            "aecho=0.8:0.6:7|19|43:0.5|0.35|0.2,volume=0.25",
            "-c:v",
            "libx264",
            "-crf",
            "23",
            "-c:a",
            "aac",
        ])
        .arg(&phone)
        .status()
        .unwrap();
    assert!(status.success());

    let linked = analyze(&phone, &AnalyzeOptions::default()).unwrap();
    let programs = ffmpeg_cli::analyze(&phone, &AnalyzeOptions::default()).unwrap();
    assert_eq!(programs.capture, CaptureMode::Camera);
    // Both read the same luma, so they agree exactly.
    assert_same(&linked, &programs, "filmed screen");
    assert!(
        programs.summary.measured > 100,
        "{}",
        programs.summary_text()
    );
    let n = programs.summary.mean_ms.unwrap();
    assert!((n - 40.0).abs() < 2.0, "mean {n}");
}

/// Video that isn't 8-bit YUV is converted on the way in, as the linked
/// decoder converts it.
#[test]
fn ten_bit_and_rgb_video_read() {
    if !have_programs() {
        return;
    }
    quiet();
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.mp4");
    generate(&options(&source, "30", 3.0, 40.0), |_| {}).unwrap();
    for (name, codec) in [
        ("ten.mp4", ["-c:v", "libx264", "-pix_fmt", "yuv420p10le"]),
        ("rgb.mp4", ["-c:v", "libx264rgb", "-pix_fmt", "rgb24"]),
    ] {
        let path = dir.path().join(name);
        let status = Command::new("ffmpeg")
            .args(["-loglevel", "error", "-y", "-i"])
            .arg(&source)
            .args(codec)
            .args(["-crf", "10", "-c:a", "copy"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success(), "{name}");
        let programs = ffmpeg_cli::analyze(&path, &AnalyzeOptions::default()).unwrap();
        assert_eq!(programs.frames.len(), 90, "{name}");
        assert_drift(&programs, 40.0, 0.5, name);
    }
}

#[test]
fn missing_files_and_streams_are_reported() {
    if !have_programs() {
        return;
    }
    let err = ffmpeg_cli::analyze("/no/such/file.mp4".as_ref(), &AnalyzeOptions::default())
        .unwrap_err()
        .to_string();
    assert!(err.contains("No such file"), "{err}");

    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("tone.wav");
    let status = Command::new("ffmpeg")
        .args([
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=duration=0.2",
        ])
        .arg(&wav)
        .status()
        .unwrap();
    assert!(status.success());
    let err = ffmpeg_cli::analyze(&wav, &AnalyzeOptions::default())
        .unwrap_err()
        .to_string();
    assert!(err.contains("no video stream"), "{err}");
}
