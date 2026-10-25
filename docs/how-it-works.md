# How it works

`av-drift analyze` answers one question at every captured video frame: *what source frame is the picture showing, and what source moment is the sound playing?* The drift is the difference.

```text
drift(t) = video_position(t) − audio_position(t)        (positive = audio late)
```

Both positions are in source frames, converted to milliseconds at the pattern's frame rate. The [test pattern specification](test-pattern.md) defines the signals referred to here.

## 1. Reading the video

Each decoded frame's luma plane is read directly (or converted to 8-bit grey first, for RGB and high-bit-depth formats). One reader goes through the capture's frames in order, so it can remember what it has seen.

1. **Barcode.** Sample the 48 cells at their fractional positions, threshold against the frame's own guard cells, and check the CRC. This takes microseconds and survives scaling, recompression, gamma and range changes. The frame number is Gray-coded, so a frame blended from two source frames differs from either in only a few cells: it reads as whichever one dominates, or fails the CRC. It never reads as some other frame.
2. **QR code.** If the barcode fails (a camera image, a rotation, a crop or letterbox, a damaged frame), look for the QR code with [`rqrr`](https://crates.io/crates/rqrr), first around where it was last seen, and only then across the whole frame. A big frame is searched downscaled to find the code, then again at full resolution around it to read it: rqrr's black-and-white threshold adapts over an eighth of the searched image's width, so a small code in a busy frame often fails to read in the whole frame.
3. **Fitting blends.** A camera whose exposure spans the display's switch from frame `M` to `M+1` records a blend: every QR module that differs between the two frames comes out grey, often too many for error correction. But once one stamp has been read, the payload format is known, so the code of every frame can be rendered exactly. The reader samples each module's brightness at full resolution, calibrates black and white on the modules that never change (finder, timing and alignment patterns, fitted as planes across the code to allow for glare), and fits it as a mix of two consecutive frames' codes, `s ≈ Q(M) + β·(Q(M+1) − Q(M))`. The best fit names the frames, and `β` is the share of the exposure after the switch. A rolling shutter exposes lower rows later, so `β` may grow linearly from the code's top row to its bottom. The barcode's cells are fitted the same way.
4. **Tracking.** Once the code is known, most frames don't need a search at all: the reader fits the code where the last frame had it, and searches again every ten frames or whenever the fit is worse than usual. A code that's still found but won't fit is decoded from its own samples, thresholded midway between its black and white. That's exactly the dominant frame's code, which a reader thresholding pixels against their surroundings mangles. If the code itself can't be read, the barcode strip is sampled through the code's perspective: the frame size in the payload fixes where the strip is relative to the code, and its cells are several times larger than the code's modules.
5. **Late reading.** Codes found before anything could be fitted are kept and read at the end, once the capture's frames are known.

The pattern's frame rate is the most common one among the stamps (barcode rate code or QR `R:` field), unless `--fps` is given. The audio tone plan comes from the QR `A:` field when present, otherwise from the frame rate. A capture of a file from av-drift 0.3 or earlier (barcode version 1, QR `AVD:3` or `AVD:2`) is reported as such.

## 2. Decoding the audio

The audio is downmixed to mono and placed on the container's timeline. Gaps in the timestamps are filled with silence, and overlaps are dropped; differences smaller than the container's timestamp resolution are treated as rounding. The detector then works through it in blocks of 8 frames.

**Alignment.** The lowest band changes tone on every frame. A window lined up with a frame contains exactly one band-0 tone; a window straddling two frames contains two. Band-0 *purity*, the strongest tone's share of the band's energy, therefore peaks at frame boundaries. For each block, the boundary phase (modulo one frame) is found by maximising purity, weighted by energy, over a coarse grid, then narrowed by a pattern search to one sample. The energy weighting stops a mostly silent window (the start of the audio, a pause) from pulling the alignment off.

**Reading and voting.** At each aligned window, a bank of 64 correlators (one per tone) picks the strongest tone in each band. A band counts only if its tone is clean (purity 0.5 or more; noise alone scores about 0.2) and at least 10 dB above the noise, measured on the six unused harmonics between the bands, where nothing ever plays. There's no fixed loudness floor: a recording 60 dB below the generated level decodes as long as its tones stand clear of its own noise. A window needs band 0 and at least one other band.

Consecutive frames must carry consecutive ids, so the block votes on which id belongs at its first frame. Each window proposes every first id it's consistent with (16 for each band it couldn't read), and the proposal most windows agree with wins; windows that disagree are dropped. So a band lost in some windows (a notch in a speaker's response, a suppressed tone) is filled in from the others. If a band is lost throughout a block, the id is known only modulo a power of 16, and the rest comes from the video frame shown at the same moment.

**Matched filter.** Room echo makes purity unreliable: the previous frame's reverb tail lingers into the next, pulling the purity peak late by a millisecond or more. But once a block's ids are known, its clean waveform is known exactly, so the detector cross-correlates it with the recording (via FFT). It does this in two passes:

1. *Coarse*: the envelope of the correlation with band 0 alone. Band 0 steps to a new tone every frame, so this correlation has one clean peak and weak side lobes. Echoes arrive later and weaker than the direct sound, so they can't win.
2. *Fine*: the full waveform's correlation within ±1.5 ms of the coarse peak, which finds the direct sound to the nearest sample.

The sign of the peak gives the recording's polarity, which matters for the next step (some microphones and audio paths invert).

**Sub-sample timing.** Every tone's phase at a window's start is `2π · f · t`, where `t` is the source time at that sample (the tones are phase-coherent across frames by construction). The phase measures `t` modulo one cycle of the tone. The matched filter has put the window within a sample or two of the boundary, so each readable band's time is taken as the one within half a cycle of zero, and the bands are combined weighted by `(f · amplitude)²`. (Unwrapping each band against the lower bands' estimate instead lets a weak or echo-biased low band pull a stronger one onto the wrong cycle, a jump of 0.4 to 1.7 ms.) The result is each audio frame's exact source time, typically to well under a microsecond on clean audio.

**Clean-up.** Cells decoded by two neighbouring blocks are de-duplicated, keeping the more certain and better-aligned copy. Finally, each cell's timing offset is compared with the median of the nine cells around it. Any that disagree by more than six times the typical scatter (plus 0.2 ms) are discarded. A codec's handling of the first frame after silence is the usual culprit. Genuine steps in drift survive, because the cells either side of a step agree with their own side.

## 3. Lining them up

The audio ids are unwrapped against the video frame number shown at about the same moment, so drift up to ±32768 frames is unambiguous (less for a cell whose id was known only modulo 16, 256 or 4096). Between decoded audio frames, the audio position is interpolated linearly, or extrapolated up to one and a half frames from the nearest when there's a gap. Positions before source time zero are discarded, because the audio is silent there.

**File or camera?** A digital copy timestamps every frame when it starts, so `t − N/fps` is the same for every frame, give or take whole frames where frames were dropped. A camera (or any capture that samples the screen at its own rate, such as a screen recording at another rate) catches frames at all phases. With consistent phases, frames caught mid-switch mean a camera; so does a QR code that moves through the capture (by more than about a module, beyond what a search's precision explains). A still QR code with consistent phases and no blends could be a digital copy or a camera on a stand locked to the pattern's rate: that's read as a copy, and the report gives the camera reading too. Barcodes only read when the picture is pixel-aligned, so they mean a copy.

**File captures.** A frame's timestamp is when it starts, so `video_position(t)` is just the frame number.

**Camera captures.** The display shows frame `N` from `a + N/fps`, and the analyzer finds the offset `a`. Camera timestamps mark the start of the exposure of the picture's top row, as phones record them.

*From blends*, when at least 5 frames (and a tenth of those read) caught the display mid-switch. A picture mixing `M` and `M+1` with weight `β` on `M+1`, exposed for `E` from `t`, saw the switch at `t + E·(1 − β)`, so

```text
a = t − (M + 1)/fps + E·(1 − β)
```

with `β` referred back to the top row through the rolling shutter's slope (the median over all frames). The exposure `E` isn't recorded in video files. When the camera's and display's phases drift apart during the capture, `β` changes with them, and a robust (Theil–Sen) line through `t − (M+1)/fps` against `1 − β` measures `E` from its slope. Otherwise `E` is only known to lie between zero and the camera's frame interval, and the analyzer takes the middle and reports the rest as uncertainty. Each blended frame gives its own switch time, so the display's own irregularities show up per frame, and frames whose switch time stands out from their neighbours' are counted as *display judder*.

*From bounds*, otherwise. Each observation of frame `N` bounds the offset: the frame on screen for most of the exposure is the one showing at its middle, `t + E/2`, so

```text
t + E/2 − (N + 1)/fps  ≤  a  ≤  t + E/2 − N/fps
```

where `E` is estimated from how often frames came out blended. Intersecting these bounds over every frame read (ignoring the most extreme 2% on each side) leaves a range of possible offsets. Its midpoint is the estimate, and its half-width is the reported *display timing* uncertainty. When the camera and pattern rates don't divide evenly, observations land at many phases and the range shrinks to a fraction of a millisecond; when they're locked, it stays a whole frame wide. If the bounds conflict (a juddering or stalling display), the analyzer falls back to the median and reports half a frame of uncertainty.

Then `video_position(t) = (t − a) · fps`.

## 4. The summary

Over all measured frames: mean, median, minimum, maximum and standard deviation of the drift, plus a least-squares line through drift against time. Its slope is the **skew**, reported as ms per minute and as parts per million (1 ms of drift per second = 1000 ppm). For file captures, gaps and repeats in the frame numbers are counted as dropped and repeated frames. The report also gives the tones' typical level and height above the noise, how frames were read, and, when few frames could be measured, why.

## Accuracy

From the test suite and `scripts/stress.py`:

| Scenario | Result |
|----------|--------|
| Synthetic audio, any rate from 24 to 120 fps, 32–96 kHz | within 2 µs per frame |
| Synthetic audio plus noise at −20 dB | within 20 µs |
| FFV1 + PCM in `.mov` (30, 29.97, 60 fps; offsets from −40 to +500 ms) | within 0.05 ms per frame |
| H.264 + AAC in `.mp4`, same grid | within 0.5 ms per frame |
| Digital copies rotated (10°, 30°, 90°), keystoned, blurred (σ up to 5), noisy, washed out, glared, shrunk to 320×180, at CRF 45, or placed small in a 4K picture | exact (40.00 ms for a true 40 ms) |
| Audio 60 dB quieter; through a small speaker's response; a notch at one tone; strong echo (60%, 45% and 30% at 23, 61 and 127 ms); AGC; AAC at 32 kb/s, Opus at 16 kb/s, MP3 at 48 kb/s | within 0.3 ms (the filters' own delay) |
| Simulated camera at 24 fps filming 25 fps, 1/50 s exposures, 12 ms rolling shutter, keystoned, soft and noisy | within 1 ms; exposure measured to ±1.5 ms |
| Simulated camera locked to the pattern's rate (30 on 30), blended | within its reported uncertainty, under ±12 ms (from ±16.7 ms by frame numbers alone) |
| `.mkv` (millisecond timestamps) | within 0.6 ms (rounding) |

## Limitations

- **Cameras locked to the pattern's rate** (30 fps filming 30 fps) are timed from the blends they catch, but their exposure time can't be measured unless the phases drift during the capture, which leaves up to about a third of a frame of uncertainty. Filming at another rate, or recording for longer, removes it (see the [camera guide](camera.md)).
- **Camera timestamps** are taken to mark the start of each exposure, as phones record them. A recorder that marks the middle shifts camera readings by half the exposure.
- **Frame-rate conversion** (e.g. 30 → 25 fps) changes which source frame is shown when. Each reading is still correct for the frame shown, but the drift includes the conversion's own timing changes.
- **Very low-bandwidth audio codecs**: at 120 fps the top tones reach 8.9 kHz, which narrowband speech codecs remove. A band lost entirely is filled in from the video, but a lower pattern rate is better for those paths.
- **Audio ids repeat** every 65536 frames (18–45 minutes depending on rate). That's harmless unless audio and video are more than half that apart.
- **The recording device's own offset** is part of any camera measurement.
