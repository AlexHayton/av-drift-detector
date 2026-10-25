use std::path::PathBuf;

/// Errors returned by av-drift-detector.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid frame rate `{0}`")]
    InvalidFrameRate(String),

    #[error("invalid time `{0}` (use seconds, `250ms`, or HH:MM:SS.mmm)")]
    InvalidTime(String),

    #[error("invalid option: {0}")]
    InvalidOption(String),

    #[error("{path}: no {kind} stream found")]
    MissingStream { path: PathBuf, kind: &'static str },

    #[error("unsupported: {0}")]
    Unsupported(String),

    #[error(
        "could not determine the test pattern's frame rate from the video; pass it explicitly (--fps)"
    )]
    UnknownFrameRate,

    #[error(
        "this capture is of a test file in an older format (from av-drift 0.3 or earlier), which this \
         version can't analyse: generate the test file again with this version, and capture it again"
    )]
    OldFormat,

    #[error("QR code: {0}")]
    Qr(String),

    /// Running the `ffmpeg` or `ffprobe` program failed (the `ffmpeg-cli` feature).
    #[cfg(feature = "ffmpeg-cli")]
    #[error("{0}")]
    FfmpegCommand(String),

    #[cfg(feature = "ffmpeg")]
    #[error("ffmpeg: {0}")]
    Ffmpeg(#[from] ffmpeg_next::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
