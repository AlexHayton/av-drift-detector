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

    #[error("QR code: {0}")]
    Qr(String),

    #[error("ffmpeg: {0}")]
    Ffmpeg(#[from] ffmpeg_next::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
