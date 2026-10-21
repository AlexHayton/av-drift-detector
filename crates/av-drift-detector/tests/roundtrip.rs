//! End to end: generate a test file, optionally mangle it with the ffmpeg CLI,
//! analyse it, and check the measured drift against the known answer.
#![cfg(feature = "ffmpeg")]

use std::path::{Path, PathBuf};
use std::process::Command;

use av_drift_detector::{
    AnalyzeOptions, AudioCodec, CaptureMode, DriftReport, FrameRate, GenerateOptions, VideoCodec,
    analyze, generate,
};

const OFFSETS_MS: [f64; 5] = [0.0, 12.3, -40.0, 78.5, 500.0];

fn quiet() {
    ffmpeg_next::log::set_level(ffmpeg_next::log::Level::Error);
}

fn make(
    dir: &Path,
    name: &str,
    fps: &str,
    offset_ms: f64,
    video: VideoCodec,
    audio: AudioCodec,
) -> PathBuf {
    quiet();
    let path = dir.join(name);
    let fps: FrameRate = fps.parse().unwrap();
    let mut opts = GenerateOptions::new(&path, fps, 3.0);
    opts.width = 640;
    opts.height = 360;
    opts.video_codec = video;
    opts.audio_codec = audio;
    opts.audio_delay = offset_ms / 1000.0;
    generate(&opts, |_| {}).unwrap();
    path
}

fn run(path: &Path) -> DriftReport {
    analyze(path, &AnalyzeOptions::default()).unwrap()
}

/// Asserts every measured frame is within `tol_ms` of `expected_ms`, and that most frames were measured.
fn assert_drift(report: &DriftReport, expected_ms: f64, tol_ms: f64, label: &str) {
    let s = &report.summary;
    let silent_frames = (expected_ms.max(0.0) / 1000.0 * report.fps.as_f64()).ceil() as usize;
    assert!(
        s.measured + silent_frames + 2 >= s.frames,
        "{label}: only {} of {} frames measured\n{}",
        s.measured,
        s.frames,
        report.summary_text()
    );
    for f in &report.frames {
        if let Some(d) = f.drift_ms {
            assert!(
                (d - expected_ms).abs() < tol_ms,
                "{label}: frame {:?} at {:.4}s drift {d:+.4} ms, expected {expected_ms:+.4} ± {tol_ms}\n{}",
                f.video_frame,
                f.time,
                report.summary_text()
            );
        }
    }
    assert_eq!(
        (s.dropped_frames, s.repeated_frames, s.no_video_stamp),
        (0, 0, 0),
        "{label}"
    );
}

fn roundtrip(video: VideoCodec, audio: AudioCodec, ext: &str, tol_ms: f64) {
    let dir = tempfile::tempdir().unwrap();
    for fps in ["30", "29.97", "60"] {
        for offset in OFFSETS_MS {
            let label = format!("{video:?}+{audio:?} {fps} fps {offset:+} ms");
            let path = make(
                dir.path(),
                &format!("t-{fps}-{offset}.{ext}"),
                fps,
                offset,
                video,
                audio,
            );
            let report = run(&path);
            assert_eq!(report.fps, fps.parse().unwrap(), "{label}");
            assert_eq!(report.fps_source, "barcode", "{label}");
            assert_drift(&report, offset, tol_ms, &label);
        }
    }
}

#[test]
fn roundtrip_h264_aac_mp4() {
    roundtrip(VideoCodec::H264, AudioCodec::Aac, "mp4", 0.5);
}

#[test]
fn roundtrip_ffv1_pcm_mov() {
    roundtrip(VideoCodec::Ffv1, AudioCodec::Pcm, "mov", 0.05);
}

#[test]
fn mkv_timestamps_are_millisecond_rounded() {
    // MKV stores times in whole milliseconds, so frames carry up to ±0.5 ms of rounding.
    let dir = tempfile::tempdir().unwrap();
    for (fps, offset) in [("29.97", 33.3), ("60", -12.0)] {
        let path = make(
            dir.path(),
            &format!("t-{fps}.mkv"),
            fps,
            offset,
            VideoCodec::Ffv1,
            AudioCodec::Pcm,
        );
        assert_drift(&run(&path), offset, 0.6, &format!("mkv {fps}"));
    }
}

#[test]
fn analysis_window() {
    let dir = tempfile::tempdir().unwrap();
    let path = make(
        dir.path(),
        "w.mp4",
        "30",
        20.0,
        VideoCodec::H264,
        AudioCodec::Aac,
    );
    let opts = AnalyzeOptions {
        offset: 1.0,
        duration: Some(1.0),
        ..Default::default()
    };
    let report = analyze(&path, &opts).unwrap();
    let ids: Vec<u32> = report.frames.iter().filter_map(|f| f.video_frame).collect();
    assert_eq!(ids, (30..60).collect::<Vec<_>>());
    assert_drift(&report, 20.0, 0.5, "window");
}

#[test]
fn rejects_bad_options() {
    let dir = tempfile::tempdir().unwrap();
    let fps: FrameRate = "30".parse().unwrap();
    let mut opts = GenerateOptions::new(dir.path().join("x.mp4"), fps, 1.0);
    opts.audio_codec = AudioCodec::Pcm;
    assert!(
        generate(&opts, |_| {}).is_err(),
        "PCM in MP4 should be refused"
    );
    let opts = GenerateOptions::new(dir.path().join("x.avi"), fps, 1.0);
    assert!(generate(&opts, |_| {}).is_err(), ".avi should be refused");
}

// ---- Robustness: real pipelines via the ffmpeg CLI ----

fn ffmpeg(args: &[&str]) -> bool {
    match Command::new("ffmpeg")
        .args(["-y", "-v", "error"])
        .args(args)
        .status()
    {
        Ok(status) => {
            assert!(status.success(), "ffmpeg {args:?} failed");
            true
        }
        Err(_) => {
            eprintln!("ffmpeg CLI not found; skipping");
            false
        }
    }
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

#[test]
fn survives_rescale_and_reencode() {
    let dir = tempfile::tempdir().unwrap();
    quiet();
    let fps: FrameRate = "30".parse().unwrap();
    let src = dir.path().join("src.mp4");
    generate(&GenerateOptions::new(&src, fps, 3.0), |_| {}).unwrap();
    let out = dir.path().join("small.mp4");
    let args = [
        "-i",
        s(&src),
        "-vf",
        "scale=640:360",
        "-c:v",
        "libx264",
        "-crf",
        "28",
        "-c:a",
        "aac",
        "-b:a",
        "96k",
        s(&out),
    ];
    if ffmpeg(&args) {
        assert_drift(&run(&out), 0.0, 1.0, "rescaled");
    }
}

#[test]
fn measures_a_pipeline_audio_shift() {
    let dir = tempfile::tempdir().unwrap();
    let src = make(
        dir.path(),
        "src.mkv",
        "25",
        0.0,
        VideoCodec::Ffv1,
        AudioCodec::Pcm,
    );
    let out = dir.path().join("shifted.mkv");
    // Delay the audio by 100 ms on remux.
    let args = [
        "-i",
        s(&src),
        "-itsoffset",
        "0.1",
        "-i",
        s(&src),
        "-map",
        "0:v",
        "-map",
        "1:a",
        "-c",
        "copy",
        s(&out),
    ];
    if ffmpeg(&args) {
        assert_drift(&run(&out), 100.0, 0.05, "itsoffset");
    }
}

#[test]
fn reports_a_dropped_frame() {
    let dir = tempfile::tempdir().unwrap();
    let src = make(
        dir.path(),
        "src.mov",
        "30",
        0.0,
        VideoCodec::Ffv1,
        AudioCodec::Pcm,
    );
    let out = dir.path().join("dropped.mov");
    // Remove frame 45 but keep every other frame's timestamp.
    let args = [
        "-i",
        s(&src),
        "-vf",
        "select='not(eq(n\\,45))'",
        "-fps_mode",
        "passthrough",
        "-c:v",
        "ffv1",
        "-c:a",
        "copy",
        s(&out),
    ];
    if ffmpeg(&args) {
        let report = run(&out);
        assert_eq!(
            report.summary.dropped_frames,
            1,
            "{}",
            report.summary_text()
        );
        let after = report
            .frames
            .iter()
            .find(|f| f.dropped_before == 1)
            .unwrap();
        assert_eq!(after.video_frame, Some(46));
        for d in report.frames.iter().filter_map(|f| f.drift_ms) {
            assert!(d.abs() < 0.05, "drift {d}");
        }
    }
}

#[test]
fn blended_frames_are_rejected_not_misread() {
    let dir = tempfile::tempdir().unwrap();
    let src = make(
        dir.path(),
        "src.mkv",
        "30",
        0.0,
        VideoCodec::Ffv1,
        AudioCodec::Pcm,
    );
    let out = dir.path().join("25.mkv");
    // `framerate` blends neighbouring frames to make new ones.
    let args = [
        "-i",
        s(&src),
        "-vf",
        "framerate=fps=25",
        "-c:v",
        "ffv1",
        "-c:a",
        "copy",
        s(&out),
    ];
    if ffmpeg(&args) {
        let report = run(&out);
        // A blend reads as its dominant source frame (or not at all); it must
        // never read as some other frame, which would be off by a whole frame (33 ms).
        assert!(report.summary.measured > 50, "{}", report.summary_text());
        // (The filter repeats the last source frame past the end of the stream.)
        for f in report.frames.iter().filter(|f| f.time < 2.95) {
            if let Some(d) = f.drift_ms {
                assert!(
                    d.abs() < 20.0,
                    "frame {:?} at {:.3}s: {d:+.2} ms",
                    f.video_frame,
                    f.time
                );
            }
        }
    }
}

#[test]
fn filmed_screen_simulation() {
    // A rough stand-in for a phone filming a screen: the picture is padded,
    // rotated and blurred (so the barcode is unreadable and the QR code must
    // be found), and sampled at an unrelated camera rate. `fps=...:round=up`
    // makes each output frame show the latest source frame that has started,
    // as a camera would. The sound picks up room echo and drops in level.
    let dir = tempfile::tempdir().unwrap();
    quiet();
    let fps: FrameRate = "30".parse().unwrap();
    let src = dir.path().join("screen.mp4");
    let mut opts = GenerateOptions::new(&src, fps, 10.0);
    opts.audio_delay = 0.040;
    generate(&opts, |_| {}).unwrap();
    let out = dir.path().join("phone.mp4");
    let vf = "pad=1600:1000:160:140:color=0x303030,rotate=0.06:fillcolor=0x303030,gblur=sigma=1.2,\
              scale=1920:1080,fps=24.93:round=up";
    let af = "aecho=0.8:0.6:7|19|43:0.5|0.35|0.2,volume=0.25";
    let args = [
        "-i",
        s(&src),
        "-vf",
        vf,
        "-af",
        af,
        "-c:v",
        "libx264",
        "-crf",
        "23",
        "-c:a",
        "aac",
        s(&out),
    ];
    if !ffmpeg(&args) {
        return;
    }
    let report = run(&out);
    assert_eq!(
        report.capture,
        CaptureMode::Camera,
        "{}",
        report.summary_text()
    );
    assert_eq!(report.fps_source, "qr");
    let s = &report.summary;
    assert!(s.measured > 200, "{}", report.summary_text());
    let u = report.video_timing_uncertainty_ms.unwrap();
    assert!(
        u < 2.0,
        "video timing ±{u:.2} ms\n{}",
        report.summary_text()
    );
    // Every frame, not just the mean: one echo (at 19 ms) once fooled a whole block.
    for f in &report.frames {
        if let Some(d) = f.drift_ms {
            assert!(
                (d - 40.0).abs() < u + 1.0,
                "frame {:?} at {:.3}s: {d:+.2} ms ±{u:.2}\n{}",
                f.video_frame,
                f.time,
                report.summary_text()
            );
        }
    }
}
