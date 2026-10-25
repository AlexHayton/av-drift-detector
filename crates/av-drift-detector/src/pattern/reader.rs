//! Reading stamps from a sequence of frames, remembering where the QR code was.

use super::canvas::LumaView;
#[cfg(feature = "qr-decode")]
use super::qr::{self, QrGrid, QrStamp};
#[cfg(feature = "qr-decode")]
use super::qr_fit::{ModuleSamples, QrTemplate, decode_samples};
use super::{FrameStamp, StampSource, barcode};
use crate::FrameRate;
use crate::media::VideoSample;

/// Candidate frames either side of the predicted one when fitting a stamp
/// that didn't decode. Covers display stalls and skips of a few frames.
const SEARCH_FRAMES: u32 = 6;

/// Codes whose modules are at least this many pixels are only located (at
/// half resolution) once a template can read them.
#[cfg(feature = "qr-decode")]
const MIN_LOCATE_MODULE_PX: f32 = 6.0;

/// While a code is tracked (fitted where it was last found, without searching
/// for it), it's searched for again at least this often, to follow the camera.
#[cfg(feature = "qr-decode")]
const TRACK_FRAMES: u32 = 10;

/// A tracked fit is kept only if its error is within this factor of the
/// last fit made where a search had just found the code; worse means the code
/// has moved.
#[cfg(feature = "qr-decode")]
const TRACK_RMS_FACTOR: f32 = 1.3;

/// Frames kept for [`StampReader::finish`] to read once more is known:
/// about a minute at 30 fps, some 5 MB.
#[cfg(feature = "qr-decode")]
const MAX_PENDING: usize = 2000;

/// Reads frame stamps from consecutive frames of one capture.
///
/// Each frame tries the barcode, then the QR code. The QR code is looked for
/// first around where it was last seen, at native resolution, which is both
/// faster and more reliable than searching a whole camera frame; only if it
/// isn't there is the whole frame searched.
///
/// Once a stamp has been read, every QR code found afterwards is also fitted
/// as a blend of two consecutive frames (see `pattern::qr_fit`). That
/// reads codes a camera caught mid-switch, which don't decode, and measures
/// how far through the switch each picture was taken. Codes found before
/// that are kept, and [`finish`](Self::finish) reads them at the end.
#[derive(Clone, Debug, Default)]
pub struct StampReader {
    #[cfg_attr(not(feature = "qr-decode"), allow(dead_code))]
    try_qr: bool,
    /// Frames read so far.
    count: usize,
    /// Where the QR code was last seen, decoded or not.
    #[cfg(feature = "qr-decode")]
    last: Option<QrGrid>,
    /// The file's QR codes, once a stamp has been read.
    #[cfg(feature = "qr-decode")]
    template: Option<QrTemplate>,
    /// The last frame identified, and when it was seen.
    anchor: Option<(f64, u32)>,
    /// The pattern's frame rate, once a stamp has told it.
    fps: Option<FrameRate>,
    /// The rate the barcode carries (`Some(None)` for a rate without a code),
    /// once a barcode has been read.
    barcode_fps: Option<Option<FrameRate>>,
    /// Codes found but not yet identified: frame index, time, grid, samples.
    #[cfg(feature = "qr-decode")]
    pending: Vec<(usize, f64, QrGrid, ModuleSamples)>,
    /// Frames read by tracking since the code was last searched for.
    #[cfg(feature = "qr-decode")]
    tracked: u32,
    /// The fit error where a search last found the code: what tracking must match.
    #[cfg(feature = "qr-decode")]
    reference_rms: Option<f32>,
}

impl StampReader {
    /// A reader that falls back to the QR code if `try_qr` (and the
    /// `qr-decode` feature) allow.
    pub fn new(try_qr: bool) -> Self {
        StampReader {
            try_qr,
            ..Default::default()
        }
    }

    /// Reads the next frame, shown at `time` (seconds, on the capture's clock).
    pub fn read(&mut self, view: &LumaView<'_>, time: f64) -> VideoSample {
        let index = self.count;
        self.count += 1;
        let sample = VideoSample {
            time,
            ..Default::default()
        };
        let (barcode, old_barcode) = self.read_barcode(view, time);
        if let Some(stamp) = barcode {
            return VideoSample {
                stamp: Some(stamp),
                ..sample
            };
        }
        let sample = VideoSample {
            old_format: old_barcode,
            ..sample
        };
        #[cfg(feature = "qr-decode")]
        if self.try_qr {
            return self.read_qr(view, index, sample);
        }
        let _ = index;
        sample
    }

    /// Reads, from what was kept, the QR codes found before enough was known
    /// to fit them, filling in `samples`: every frame this reader returned,
    /// in order.
    pub fn finish(&mut self, samples: &mut [VideoSample]) {
        #[cfg(feature = "qr-decode")]
        {
            let Some(template) = self.template.as_mut() else {
                return;
            };
            let fps = template.stamp(0).fps;
            for (index, time, grid, raw) in std::mem::take(&mut self.pending) {
                if samples.get(index).is_none_or(|s| s.stamp.is_some()) {
                    continue;
                }
                // Predict from the nearest frame already identified.
                let nearest = samples
                    .iter()
                    .filter_map(|s| s.stamp.map(|st| (s.time, st.frame_id)))
                    .min_by(|a, b| (a.0 - time).abs().total_cmp(&(b.0 - time).abs()));
                let Some(nearest) = nearest else {
                    return;
                };
                let Some(predicted) = predict(nearest, time, fps) else {
                    continue;
                };
                let candidates =
                    predicted.saturating_sub(SEARCH_FRAMES)..=predicted + SEARCH_FRAMES;
                if let Some(blend) = template.fit_samples(&raw, candidates) {
                    samples[index].stamp = Some(blend_stamp(template, &grid, blend));
                }
            }
        }
        let _ = samples;
    }

    /// The barcode, read outright or, if a blend of two frames broke its
    /// CRC, fitted as one; and whether it's an earlier release's barcode.
    fn read_barcode(&mut self, view: &LumaView<'_>, time: f64) -> (Option<FrameStamp>, bool) {
        match barcode::read(view) {
            Err(barcode::BarcodeError::Version(v)) if v < barcode::VERSION => (None, true),
            read => (self.barcode_stamp(view, time, read), false),
        }
    }

    fn barcode_stamp(
        &mut self,
        view: &LumaView<'_>,
        time: f64,
        read: Result<barcode::BarcodeStamp, barcode::BarcodeError>,
    ) -> Option<FrameStamp> {
        let stamp = |frame_id, fps, source, blend| FrameStamp {
            frame_id,
            fps,
            tone_base_harmonic: None,
            source,
            corners: None,
            blend,
        };
        match read {
            Ok(read) => {
                let id = read.frame_id;
                self.anchor = Some((time, id));
                self.barcode_fps = Some(read.fps);
                self.fps = read.fps.or(self.fps);
                #[cfg(feature = "qr-decode")]
                self.seed_from_barcode(id, read.fps, (view.width() as u32, view.height() as u32));
                let blend = barcode::sample(view)
                    .and_then(|cells| {
                        barcode::fit_blend(&cells, read.fps, id.saturating_sub(2)..=id + 1)
                    })
                    .filter(|b| b.dominant() == id);
                Some(stamp(id, read.fps, StampSource::Barcode, blend))
            }
            Err(_) => {
                let (fps, barcode_fps) = (self.fps?, self.barcode_fps?);
                let cells = barcode::sample(view)?;
                let predicted = predict(self.anchor?, time, fps)?;
                let blend = barcode::fit_blend(
                    &cells,
                    barcode_fps,
                    predicted.saturating_sub(SEARCH_FRAMES)..=predicted + SEARCH_FRAMES,
                )?;
                let id = blend.dominant();
                self.anchor = Some((time, id));
                Some(stamp(
                    id,
                    barcode_fps,
                    StampSource::BarcodeBlend,
                    Some(blend),
                ))
            }
        }
    }

    /// A barcode carries the frame number and rate, and the generator always
    /// uses the standard tone plan for the rate. If the picture is the test
    /// file's own size, that's enough to render its QR codes too.
    #[cfg(feature = "qr-decode")]
    fn seed_from_barcode(&mut self, frame_id: u32, fps: Option<FrameRate>, frame_size: (u32, u32)) {
        if !self.try_qr {
            return;
        }
        if self.template.is_none()
            && let Some(fps) = fps
        {
            let stamp = QrStamp {
                frame_id,
                fps,
                tone_base_harmonic: crate::audio::ToneCode::default_base_harmonic(fps),
                frame_size,
            };
            self.template = QrTemplate::new(&stamp).ok();
        }
    }

    /// Fits the code where the last frame had it, without searching for it:
    /// a camera moves little between frames, and the fit tolerates a pixel or
    /// two. Gives up (to a search) every few frames, or once the fit is worse
    /// than usual.
    #[cfg(feature = "qr-decode")]
    fn track(&mut self, view: &LumaView<'_>, time: f64) -> Option<FrameStamp> {
        if self.tracked >= TRACK_FRAMES {
            return None;
        }
        let (template, grid, anchor) = (self.template.as_mut()?, self.last?, self.anchor?);
        let reference = self.reference_rms?;
        let predicted = predict(anchor, time, template.stamp(0).fps)?;
        let blend = template.fit(
            view,
            &grid,
            predicted.saturating_sub(SEARCH_FRAMES)..=predicted + SEARCH_FRAMES,
        )?;
        if blend.rms > TRACK_RMS_FACTOR * reference.max(0.02) {
            return None;
        }
        self.tracked += 1;
        self.anchor = Some((time, blend.dominant()));
        Some(blend_stamp(template, &grid, blend))
    }

    #[cfg(feature = "qr-decode")]
    fn read_qr(&mut self, view: &LumaView<'_>, index: usize, sample: VideoSample) -> VideoSample {
        if let Some(stamp) = self.track(view, sample.time) {
            return VideoSample {
                stamp: Some(stamp),
                qr_located: true,
                ..sample
            };
        }
        self.tracked = 0;
        // With a template, every code is read by fitting it at full
        // resolution, so the search only has to find it, which it can do
        // faster at half resolution (if the modules are big enough to survive that).
        let effort = match (&self.template, self.last) {
            (Some(_), Some(last)) if last.side() / last.modules as f32 >= MIN_LOCATE_MODULE_PX => {
                qr::Effort::Locate
            }
            _ => qr::Effort::Decode,
        };
        let near = self
            .last
            .map(|last| qr::search_with(view, Some(last.region(qr::REGION_MARGIN)), effort))
            .filter(|s| s.found.is_some() || !s.undecoded.is_empty());
        let search = near.unwrap_or_else(|| qr::search_with(view, None, qr::Effort::Decode));
        let sample = VideoSample {
            old_format: sample.old_format || search.old_format,
            ..sample
        };
        if let Some(found) = search.found {
            let stamp = self.decoded(view, &found, sample.time);
            return VideoSample {
                stamp: Some(stamp),
                qr_located: true,
                ..sample
            };
        }
        let Some(&grid) = search.undecoded.first() else {
            // Not found at all (glare, blur): the barcode, through where the
            // code last was, may still read.
            if let Some(stamp) = self
                .last
                .and_then(|last| self.barcode_through(view, &last, sample.time))
            {
                return VideoSample {
                    stamp: Some(stamp),
                    ..sample
                };
            }
            return sample;
        };
        self.last = Some(grid);
        let located = VideoSample {
            qr_located: true,
            ..sample
        };
        let Some(raw) = ModuleSamples::take(view, &grid) else {
            return located;
        };
        let fitted = match (self.template.as_mut(), self.anchor) {
            (Some(template), Some(anchor)) => predict(anchor, sample.time, template.stamp(0).fps)
                .and_then(|predicted| {
                    template.fit_samples(
                        &raw,
                        predicted.saturating_sub(SEARCH_FRAMES)..=predicted + SEARCH_FRAMES,
                    )
                })
                .map(|blend| blend_stamp(template, &grid, blend)),
            _ => None,
        };
        // The fit couldn't (no template yet, or the frame wasn't where it was
        // expected): decode the samples, thresholded against the code's own
        // black and white.
        let fitted = fitted.or_else(|| {
            let stamp = decode_samples(&raw)?;
            let found = qr::LocatedQr {
                stamp,
                corners: grid.corners,
                modules: grid.modules,
            };
            Some(self.decoded(view, &found, sample.time))
        });
        match fitted {
            Some(stamp) => {
                self.anchor = Some((sample.time, stamp.frame_id));
                self.reference_rms = stamp.blend.map(|b| b.rms);
                VideoSample {
                    stamp: Some(stamp),
                    ..located
                }
            }
            None => match self.barcode_through(view, &grid, sample.time) {
                Some(stamp) => VideoSample {
                    stamp: Some(stamp),
                    ..located
                },
                None => {
                    if self.pending.len() < MAX_PENDING {
                        self.pending.push((index, sample.time, grid, raw));
                    }
                    located
                }
            },
        }
    }

    /// The stamp for a decoded code, with the template brought up to date and
    /// the mix of frames measured.
    #[cfg(feature = "qr-decode")]
    fn decoded(&mut self, view: &LumaView<'_>, found: &qr::LocatedQr, time: f64) -> FrameStamp {
        let grid = found.grid();
        self.last = Some(grid);
        let template = match self.template.take() {
            Some(t) if t.matches(&found.stamp) => Some(t),
            _ => QrTemplate::new(&found.stamp).ok(),
        };
        self.template = template;
        let id = found.stamp.frame_id;
        self.anchor = Some((time, id));
        self.fps = Some(found.stamp.fps);
        // The decode names the frame; the fit measures the mix. Keep the
        // fit only if it agrees.
        let blend = self.template.as_mut().and_then(|t| {
            t.fit(
                view,
                &grid,
                id.saturating_sub(SEARCH_FRAMES)..=id + SEARCH_FRAMES,
            )
            .filter(|b| b.dominant() == id)
        });
        self.reference_rms = blend.map(|b| b.rms);
        FrameStamp {
            blend,
            ..FrameStamp::from_qr(found)
        }
    }

    /// The barcode strip read through the QR code's perspective at `grid`:
    /// its cells are several times the size of the code's modules, so it can
    /// read a frame whose code is too blurred, small or glared to fit.
    #[cfg(feature = "qr-decode")]
    fn barcode_through(
        &mut self,
        view: &LumaView<'_>,
        grid: &QrGrid,
        time: f64,
    ) -> Option<FrameStamp> {
        let template = self.template.as_ref()?;
        let predicted = predict(self.anchor?, time, template.stamp(0).fps)?;
        let blend = template.fit_barcode(
            view,
            grid,
            predicted.saturating_sub(SEARCH_FRAMES)..=predicted + SEARCH_FRAMES,
        )?;
        let id = blend.dominant();
        self.anchor = Some((time, id));
        let stamp = template.stamp(id);
        Some(FrameStamp {
            frame_id: id,
            fps: Some(stamp.fps),
            tone_base_harmonic: Some(stamp.tone_base_harmonic),
            source: StampSource::BarcodeBlend,
            corners: Some(grid.corners),
            blend: Some(blend),
        })
    }
}

/// The frame on screen at `time`, from frame `anchor.1` seen at `anchor.0`.
fn predict(anchor: (f64, u32), time: f64, fps: FrameRate) -> Option<u32> {
    let predicted = f64::from(anchor.1) + ((time - anchor.0) * fps.as_f64()).round();
    (0.0..f64::from(u32::MAX - 2 * SEARCH_FRAMES))
        .contains(&predicted)
        .then_some(predicted as u32)
}

/// The stamp for a code read as a blend.
#[cfg(feature = "qr-decode")]
fn blend_stamp(template: &QrTemplate, grid: &QrGrid, blend: super::Blend) -> FrameStamp {
    let id = blend.dominant();
    let stamp = template.stamp(id);
    FrameStamp {
        frame_id: id,
        fps: Some(stamp.fps),
        tone_base_harmonic: Some(stamp.tone_base_harmonic),
        source: StampSource::QrBlend,
        corners: Some(grid.corners),
        blend: Some(blend),
    }
}
