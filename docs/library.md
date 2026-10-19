# Using the library

```toml
[dependencies]
av-drift-detector = { version = "0.1", default-features = false, features = ["qr-decode"] }
```

| Feature | Default | What it adds |
|---------|---------|--------------|
| `qr-decode` | yes | Reading QR codes (via [`rqrr`](https://crates.io/crates/rqrr)): the fallback for damaged barcodes, and how camera captures are read. |
| `cli` | yes | The `av-drift` command and its dependencies (`clap`, `indicatif`, `serde_json`, `anyhow`). Turn it off when using the library. |

Building needs FFmpeg's development libraries; see [Getting started](getting-started.md#install). The minimum Rust version is 1.88.

## Generate a test file

```rust,no_run
use av_drift_detector::{AudioCodec, GenerateOptions, VideoCodec, generate};

fn main() -> Result<(), av_drift_detector::Error> {
    let fps = "59.94".parse()?;
    let mut opts = GenerateOptions::new("reference.mov", fps, 30.0);
    opts.width = 1920;
    opts.height = 1080;
    opts.video_codec = VideoCodec::Ffv1;
    opts.audio_codec = AudioCodec::Pcm;
    opts.audio_delay = -0.010; // audio 10 ms early

    generate(&opts, |p| {
        if p.frames_done % 600 == 0 {
            eprintln!("{} / {}", p.frames_done, p.frames_total);
        }
    })
}
```

## Analyze a capture

```rust,no_run
use av_drift_detector::{AnalyzeOptions, CaptureMode, FrameStatus, analyze};

fn main() -> Result<(), av_drift_detector::Error> {
    let opts = AnalyzeOptions {
        offset: 5.0,          // skip the first 5 s
        duration: Some(20.0), // and analyze 20 s
        capture: CaptureMode::Auto,
        ..Default::default()
    };
    let report = analyze("captured.mp4".as_ref(), &opts)?;

    println!("{}", report.summary_text());
    if let Some(skew) = report.summary.skew_ppm {
        println!("audio clock differs from video by {skew:+.1} ppm");
    }
    for f in report.frames.iter().filter(|f| f.status == FrameStatus::Ok) {
        println!("{:.3}s frame {} drift {:+.3} ms", f.time, f.video_frame.unwrap(), f.drift_ms.unwrap());
    }

    // The report is `serde::Serialize`.
    std::fs::write("report.csv", report.to_csv())?;
    Ok(())
}
```

`DriftReport` has `summary_text()`, `frames_table()` and `to_csv()` for the CLI's text formats. It also serializes to the JSON shown in the [CLI reference](cli.md#json---json).

## Bring your own decoder

`analyze` decodes with FFmpeg. If you already have frames and audio from somewhere else (a capture card SDK, WebRTC, GStreamer), build a `MediaScan` and call `analyze_scan`:

```rust
use av_drift_detector::analyze::{AnalyzeOptions, analyze_scan};
use av_drift_detector::media::decode::{AudioTrack, MediaScan, VideoSample};
use av_drift_detector::pattern::canvas::LumaView;
use av_drift_detector::pattern::decode_frame;

fn measure(frames: &[(f64, Vec<u8>)], width: usize, height: usize, audio: Vec<f32>, audio_start: f64)
    -> Result<av_drift_detector::DriftReport, av_drift_detector::Error>
{
    let video = frames
        .iter()
        .map(|(time, luma)| VideoSample {
            time: *time,
            // An 8-bit luma (Y) plane; stride == width here.
            stamp: decode_frame(&LumaView::new(luma, width, height, width), true),
        })
        .collect();
    let scan = MediaScan {
        video,
        audio: Some(AudioTrack { sample_rate: 48_000, start_time: audio_start, samples: audio }),
        width: width as u32,
        height: height as u32,
        stream_fps: None,
    };
    analyze_scan(&scan, &AnalyzeOptions::default(), "my capture".into())
}
```

Timestamps for video and audio must share one clock (seconds). Audio is mono `f32` at any sample rate.

## Use the pattern without FFmpeg

The rendering and the audio code are plain Rust. Use them to build test signals into another tool, a browser via WebAssembly, or a hardware signal generator.

**Render frames** into any 8-bit luma plane, such as the Y plane of a YUV frame. Use luma 128 in the chroma planes for a grey-scale picture.

```rust
use av_drift_detector::audio::ToneCode;
use av_drift_detector::pattern::canvas::LumaImage;
use av_drift_detector::pattern::PatternRenderer;

let tones = ToneCode::new("30".parse().unwrap(), 48_000).unwrap();
let renderer = PatternRenderer::new(1280, 720, tones).unwrap();
let mut frame = LumaImage::new(1280, 720, 0);
renderer.render(42, &mut frame.canvas()).unwrap();
// frame.data is now frame 42: barcode, timecode, frame number, progress ticks, QR code.
```

`LumaCanvas::new(data, width, height, stride)` wraps a plane you already own, with any stride.

**Synthesize the audio** sample by sample:

```rust
use av_drift_detector::audio::{ToneCode, ToneSynth};

let code = ToneCode::new("30".parse().unwrap(), 48_000).unwrap();
let mut synth = ToneSynth::new(code, 0.0); // or a deliberate delay in seconds
let mut buffer = vec![0.0f32; 1024];
synth.fill(&mut buffer); // call repeatedly; the stream is continuous
```

**Decode audio** without the rest of the analysis:

```rust
use av_drift_detector::audio::{CellDetector, ToneCode};

let code = ToneCode::new("30".parse().unwrap(), 48_000).unwrap();
let detector = CellDetector::new(code);
// One second of test audio; in practice, your own mono recording.
let samples = av_drift_detector::audio::synthesize(detector.code(), 0.0, 0, 48_000);
for cell in detector.detect(&samples, 0.0) {
    // At `cell.time` (seconds, your clock) the audio is at source position
    // frame `cell.id` (mod 65536) plus `cell.residual` seconds.
    println!("{:.6}s: frame {} {:+.1} µs", cell.time, cell.id, cell.residual * 1e6);
}
```

## Errors

Everything returns `av_drift_detector::Result<T>`, with `av_drift_detector::Error` covering:
- invalid options (`InvalidOption`, `InvalidFrameRate`, `InvalidTime`);
- unsupported input (`MissingStream`, `Unsupported`);
- a capture whose frame rate can't be determined (`UnknownFrameRate`);
- FFmpeg and I/O failures (`Ffmpeg`, `Io`).
