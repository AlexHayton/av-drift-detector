//! Pictures that caught the display switching between two frames.

/// A picture read as a blend of two consecutive frames: a camera whose
/// exposure spanned the display's switch, or a capture that mixed frames.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Blend {
    /// The earlier of the two frames: the picture is a mix of `first` and `first + 1`.
    pub first: u32,
    /// The weight of `first + 1` at the stamp's middle row, from 0 to 1: the
    /// share of that row's exposure that came after the display switched.
    pub weight_next: f32,
    /// How much the weight grows from the stamp's top row to its bottom row
    /// (a rolling shutter exposes lower rows later). Zero for the barcode.
    pub slope: f32,
    /// Root-mean-square error of the fit (black 0, white 1).
    pub rms: f32,
    /// The RMS error of the best fit naming a different frame, over `rms`.
    pub margin: f32,
}

impl Blend {
    /// The frame that covered most of the exposure.
    pub fn dominant(&self) -> u32 {
        if self.weight_next < 0.5 {
            self.first
        } else {
            self.first + 1
        }
    }

    /// Whether both frames are visibly present.
    pub fn is_mixed(&self) -> bool {
        (0.05..=0.95).contains(&self.weight_next)
    }
}

/// Fits worse than this (root-mean-square error, with black at 0 and white
/// at 1) aren't a reading of the stamp.
pub const MAX_RMS: f32 = 0.2;
/// The best fit must beat the best one that names a different frame by this
/// factor in RMS error.
pub const MIN_MARGIN: f32 = 1.25;

/// Chooses among fits of candidate frame pairs: the best, if it's good and
/// clearly better than any fit that names another frame.
pub(crate) fn choose(fits: &[Blend]) -> Option<Blend> {
    let best = fits.iter().min_by(|a, b| a.rms.total_cmp(&b.rms))?;
    let runner = fits
        .iter()
        .filter(|f| f.dominant() != best.dominant())
        .map(|f| f.rms)
        .fold(f32::INFINITY, f32::min);
    let mut fit = Blend {
        margin: runner / best.rms.max(1e-6),
        ..*best
    };
    // A picture of one frame fits as either neighbouring pair; name it the
    // same way whichever won: that frame, with nothing of the next.
    if !fit.is_mixed() && fit.weight_next >= 0.5 {
        fit.first += 1;
        fit.weight_next = 0.0;
    }
    (fit.rms < MAX_RMS && fit.margin >= MIN_MARGIN).then_some(fit)
}
