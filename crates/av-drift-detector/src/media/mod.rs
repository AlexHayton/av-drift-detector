//! Captures as the analyser sees them, test-file settings, and FFmpeg input
//! and output: linked through `ffmpeg-next` (the `ffmpeg` feature, modules
//! `decode` and `encode`), or by running the `ffmpeg` and `ffprobe` programs
//! (the `ffmpeg-cli` feature, module `ffmpeg_cli`).
//!
//! The types here are plain data, so other decoders (Android's MediaCodec, a
//! capture card SDK, a live camera and microphone) can build a [`MediaScan`]
//! and pass it to [`analyze_scan`](crate::analyze::analyze_scan).

#[cfg(feature = "ffmpeg")]
pub mod decode;
#[cfg(feature = "ffmpeg")]
pub mod encode;
#[cfg(feature = "ffmpeg-cli")]
pub mod ffmpeg_cli;

use std::path::PathBuf;
use std::str::FromStr;

use crate::pattern::FrameStamp;
use crate::{Error, FrameRate, Result};

/// One decoded video frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct VideoSample {
    /// Presentation time in seconds (container timeline). For a camera, the
    /// start of the exposure of the picture's top row, as phones record it.
    pub time: f64,
    /// The stamp read from the frame, if any.
    pub stamp: Option<FrameStamp>,
    /// Whether a QR code was found in the frame, even if it couldn't be read.
    pub qr_located: bool,
    /// Whether the frame carries a stamp in an earlier release's format,
    /// which isn't read.
    pub old_format: bool,
    /// Camera frames: how long the picture was exposed, in seconds, if the
    /// camera said (phones' capture metadata does). Video files don't record
    /// it, and the analyzer then works it out from the frames, less exactly.
    pub exposure: Option<f64>,
    /// Camera frames: the rolling shutter's readout, in seconds, if the camera
    /// said: row `y` of a picture `height` rows tall began its exposure
    /// `readout · y / height` after [`time`](Self::time) (Android's
    /// `SENSOR_ROLLING_SHUTTER_SKEW`).
    pub readout: Option<f64>,
}

/// Decoded audio, downmixed to mono.
#[derive(Clone, Debug)]
pub struct AudioTrack {
    pub sample_rate: u32,
    /// Container time of `samples[0]`.
    pub start_time: f64,
    pub samples: Vec<f32>,
}

/// Everything the analyser needs from a file.
#[derive(Clone, Debug)]
pub struct MediaScan {
    pub video: Vec<VideoSample>,
    pub audio: Option<AudioTrack>,
    pub width: u32,
    pub height: u32,
    /// The video stream's average frame rate, if the container reports one.
    pub stream_fps: Option<FrameRate>,
}

/// Which part of the file to read.
#[derive(Clone, Copy, Debug)]
pub struct ScanWindow {
    /// Video frames before this time (seconds) are skipped.
    pub start: f64,
    /// Video frames at or after this time are skipped; `None` reads to the end.
    pub end: Option<f64>,
    /// Extra audio to keep on either side of the video window, in seconds.
    pub audio_margin: f64,
}

/// Video codec for generated files.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VideoCodec {
    /// H.264 via libx264 (CRF 18), the most portable choice.
    #[default]
    H264,
    /// FFV1, lossless (.mkv or .mov only).
    Ffv1,
}

/// Audio codec for generated files.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AudioCodec {
    /// AAC at 256 kb/s.
    #[default]
    Aac,
    /// Uncompressed 16-bit PCM (.mkv or .mov only).
    Pcm,
}

impl FromStr for VideoCodec {
    type Err = Error;

    /// `h264` or `ffv1`.
    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "h264" => Ok(VideoCodec::H264),
            "ffv1" => Ok(VideoCodec::Ffv1),
            _ => Err(Error::InvalidOption(format!(
                "video codec must be h264 or ffv1, not `{s}`"
            ))),
        }
    }
}

impl FromStr for AudioCodec {
    type Err = Error;

    /// `aac` or `pcm`.
    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "aac" => Ok(AudioCodec::Aac),
            "pcm" => Ok(AudioCodec::Pcm),
            _ => Err(Error::InvalidOption(format!(
                "audio codec must be aac or pcm, not `{s}`"
            ))),
        }
    }
}

/// Settings for writing a test file, with `encode::generate` (the `ffmpeg`
/// feature) or `ffmpeg_cli::generate` (the `ffmpeg-cli` feature).
#[derive(Clone, Debug)]
pub struct GenerateOptions {
    pub output: PathBuf,
    pub fps: FrameRate,
    /// Duration in seconds, rounded to whole frames.
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    pub sample_rate: u32,
    pub video_codec: VideoCodec,
    pub audio_codec: AudioCodec,
    /// A deliberate audio delay in seconds (positive = audio late, negative = early),
    /// to check an analyser or a pipeline against a known offset.
    pub audio_delay: f64,
    /// Mix the [`Music`](crate::audio::Music) in under the code (up to about
    /// 60 fps). It doesn't affect analysis.
    pub music: bool,
}

impl GenerateOptions {
    /// 1280×720, 48 kHz, H.264 + AAC, no injected delay, with music.
    pub fn new(output: impl Into<PathBuf>, fps: FrameRate, duration: f64) -> Self {
        GenerateOptions {
            output: output.into(),
            fps,
            duration,
            width: 1280,
            height: 720,
            sample_rate: 48000,
            video_codec: VideoCodec::default(),
            audio_codec: AudioCodec::default(),
            audio_delay: 0.0,
            music: true,
        }
    }

    pub fn total_frames(&self) -> u64 {
        self.fps.frames_in(self.duration)
    }

    /// Checks the options before anything is written.
    #[cfg(any(feature = "ffmpeg", feature = "ffmpeg-cli"))]
    pub(crate) fn validate(&self) -> Result<()> {
        use crate::pattern::barcode::MAX_FRAME_ID;
        let opts = self;
        let invalid = |msg: String| Err(Error::InvalidOption(msg));
        if !(opts.duration.is_finite() && opts.duration > 0.0) {
            return invalid(format!("duration must be positive, got {}", opts.duration));
        }
        if opts.total_frames() > u64::from(MAX_FRAME_ID) + 1 {
            return invalid("duration exceeds 2^24 frames".into());
        }
        if !(8000..=192_000).contains(&opts.sample_rate) {
            return invalid(format!("sample rate {} is out of range", opts.sample_rate));
        }
        if !opts.audio_delay.is_finite() || opts.audio_delay.abs() > 60.0 {
            return invalid("audio delay must be within ±60 s".into());
        }
        let ext = opts
            .output
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        let flexible = matches!(ext.as_str(), "mkv" | "mov");
        if !flexible && !matches!(ext.as_str(), "mp4" | "m4v") {
            return invalid(format!(
                "unsupported output container `.{ext}` (use .mp4, .mov or .mkv)"
            ));
        }
        if !flexible && opts.audio_codec == AudioCodec::Pcm {
            return invalid("PCM audio needs a .mov or .mkv output".into());
        }
        if !flexible && opts.video_codec == VideoCodec::Ffv1 {
            return invalid("FFV1 video needs a .mov or .mkv output".into());
        }
        Ok(())
    }
}

/// Generation progress, reported once per video frame.
#[derive(Clone, Copy, Debug)]
pub struct Progress {
    pub frames_done: u64,
    pub frames_total: u64,
}

/// Assembles one continuous [`AudioTrack`] from timestamped chunks of mono
/// audio, as a decoder delivers them.
///
/// The track's timeline stays continuous: gaps between chunks are filled with
/// silence and overlaps are dropped. Mismatches up to `tolerance` seconds are
/// treated as timestamp rounding, not real gaps.
#[derive(Clone, Debug)]
pub struct AudioTrackBuilder {
    from: f64,
    to: Option<f64>,
    tolerance: f64,
    track: Option<AudioTrack>,
}

impl AudioTrackBuilder {
    /// Keeps audio from container time `from` until `to` (`None`: no limit).
    pub fn new(from: f64, to: Option<f64>, tolerance: f64) -> Self {
        AudioTrackBuilder {
            from,
            to,
            tolerance,
            track: None,
        }
    }

    /// Appends `data`, whose first sample is at container time `time`.
    pub fn push(&mut self, time: f64, rate: u32, data: &[f32]) -> Result<()> {
        let r = f64::from(rate);
        let track = match &mut self.track {
            Some(track) => {
                if track.sample_rate != rate {
                    return Err(Error::Unsupported(
                        "audio sample rate changes mid-stream".into(),
                    ));
                }
                track
            }
            None => {
                let skip = ((self.from - time) * r).ceil().max(0.0) as usize;
                if skip >= data.len() {
                    return Ok(());
                }
                self.track = Some(AudioTrack {
                    sample_rate: rate,
                    start_time: time + skip as f64 / r,
                    samples: data[skip..].to_vec(),
                });
                self.trim();
                return Ok(());
            }
        };
        let mismatch = time - track.start_time - track.samples.len() as f64 / r;
        let gap = if mismatch.abs() <= self.tolerance {
            0
        } else {
            (mismatch * r).round() as i64
        };
        let data = if gap > 0 {
            track
                .samples
                .resize(track.samples.len() + gap as usize, 0.0);
            data
        } else {
            &data[((-gap) as usize).min(data.len())..]
        };
        track.samples.extend_from_slice(data);
        self.trim();
        Ok(())
    }

    /// Whether the track has reached its end time.
    pub fn is_full(&self) -> bool {
        match (self.to, &self.track) {
            (Some(to), Some(track)) => {
                track.samples.len() as f64 >= (to - track.start_time) * f64::from(track.sample_rate)
            }
            _ => false,
        }
    }

    /// The track so far.
    pub fn track(&self) -> Option<&AudioTrack> {
        self.track.as_ref()
    }

    /// The finished track, or `None` if no audio fell within the range.
    pub fn finish(self) -> Option<AudioTrack> {
        self.track
    }

    fn trim(&mut self) {
        let (Some(to), Some(track)) = (self.to, self.track.as_mut()) else {
            return;
        };
        let keep = ((to - track.start_time) * f64::from(track.sample_rate))
            .ceil()
            .max(0.0) as usize;
        track.samples.truncate(keep);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_a_continuous_track() {
        let mut b = AudioTrackBuilder::new(0.5, Some(2.0), 0.002);
        // Starts before `from`: the first half second is skipped.
        b.push(0.0, 1000, &[1.0; 1000]).unwrap();
        assert_eq!(b.track().unwrap().start_time, 0.5);
        assert_eq!(b.track().unwrap().samples.len(), 500);
        // A 1 ms rounding error is ignored.
        b.push(1.001, 1000, &[2.0; 100]).unwrap();
        assert_eq!(b.track().unwrap().samples.len(), 600);
        // A 50 ms gap is filled with silence.
        b.push(1.15, 1000, &[3.0; 100]).unwrap();
        let t = b.track().unwrap();
        assert_eq!(t.samples.len(), 750);
        assert_eq!((t.samples[620], t.samples[660]), (0.0, 3.0));
        // A 20 ms overlap is dropped.
        b.push(1.23, 1000, &[4.0; 100]).unwrap();
        assert_eq!(b.track().unwrap().samples.len(), 830);
        assert!(!b.is_full());
        // Stops at `to`.
        b.push(1.33, 1000, &[5.0; 1000]).unwrap();
        assert!(b.is_full());
        assert_eq!(b.finish().unwrap().samples.len(), 1500);
    }

    #[test]
    fn rejects_a_rate_change() {
        let mut b = AudioTrackBuilder::new(0.0, None, 0.002);
        b.push(0.0, 48000, &[0.0; 10]).unwrap();
        assert!(b.push(0.1, 44100, &[0.0; 10]).is_err());
    }
}
