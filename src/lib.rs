//! Generate machine-readable A/V sync test videos and measure audio/video drift.
//!
//! Every video frame of a generated file carries its frame number three ways:
//! a 48-bit barcode strip (with a CRC), a QR code, and a seven-segment
//! timecode for people. Every frame's worth of audio carries the same number
//! (mod 65536) as four simultaneous tones. Analysis decodes both sides of a
//! captured or re-encoded copy and reports, frame by frame, how far the audio
//! has drifted from the video. Precision is a small fraction of a millisecond.
//!
//! ```no_run
//! use av_drift_detector::{AnalyzeOptions, GenerateOptions, analyze, generate};
//!
//! let fps = "29.97".parse()?;
//! generate(&GenerateOptions::new("sync.mp4", fps, 10.0), |_| {})?;
//! // ... play `sync.mp4` through the system under test and record it ...
//! let report = analyze("captured.mp4".as_ref(), &AnalyzeOptions::default())?;
//! println!("{}", report.summary_text());
//! # Ok::<(), av_drift_detector::Error>(())
//! ```
//!
//! The pattern rendering ([`pattern`]) and audio code ([`audio`]) are plain Rust
//! with no FFmpeg dependency, for use with other I/O stacks.

pub mod analyze;
pub mod audio;
mod error;
pub mod media;
pub mod pattern;
pub mod report;
pub mod timing;

pub use analyze::{AnalyzeOptions, analyze};
pub use error::{Error, Result};
pub use media::encode::{AudioCodec, GenerateOptions, Progress, VideoCodec, generate};
pub use report::{CaptureMode, DriftReport, DriftSummary, FrameDrift, FrameStatus};
pub use timing::{FrameRate, parse_time};

// Compile the examples in the README and the library guide as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

#[cfg(doctest)]
#[doc = include_str!("../docs/library.md")]
struct LibraryGuideDoctests;
