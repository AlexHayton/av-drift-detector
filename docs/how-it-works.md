# How it works

`av-drift analyze` answers one question at every captured video frame: *what source frame is the picture showing, and what source moment is the sound playing?* The drift is the difference.

```text
drift(t) = video_position(t) − audio_position(t)        (positive = audio late)
```

Both positions are in source frames, converted to milliseconds at the pattern's frame rate. The [test pattern specification](test-pattern.md) defines the signals referred to here.

## 1. Reading the video

Each decoded frame's luma plane is read directly (or converted to 8-bit grey first, for RGB and high-bit-depth formats).

1. **Barcode.** Sample the 48 cells at their fractional positions, threshold against the frame's own guard cells, and check the CRC. This takes microseconds and survives scaling, recompression, gamma and range changes. A frame blended from two source frames reads as whichever one dominates, or fails the CRC; it never reads as some other frame.
2. **QR code.** If the barcode fails (a camera image, a crop or letterbox, a damaged frame), search the frame for the QR code with [`rqrr`](https://crates.io/crates/rqrr). Frames wider than 1280 pixels are box-downscaled first.

The pattern's frame rate is the most common one among the stamps (barcode rate code or QR `R:` field), unless `--fps` is given. The audio tone plan comes from the QR `A:` field when present, otherwise from the frame rate.

## 2. Decoding the audio

The audio is downmixed to mono and placed on the container's timeline. Gaps in the timestamps are filled with silence, and overlaps are dropped; differences smaller than the container's timestamp resolution are treated as rounding. The detector then works through it in blocks of 8 frames.

**Alignment.** The lowest band changes tone on every frame. A window lined up with a frame contains exactly one band-0 tone; a window straddling two frames contains two. Band-0 *purity*, the strongest tone's share of the band's energy, therefore peaks at frame boundaries. For each block, the boundary phase (modulo one frame) is found by maximising purity, weighted by energy, over a coarse grid, then narrowed by a pattern search to one sample. The energy weighting stops a mostly silent window (the start of the audio, a pause) from pulling the alignment off.

**Decoding and voting.** At each aligned window, a bank of 64 correlators (one per tone) picks the strongest tone in each band, giving the four nibbles of the 16-bit id. Consecutive frames must carry consecutive ids, so the block votes on which id belongs at its first frame; cells that disagree are dropped. Cells are also dropped when too quiet (40 dB below a generated tone, about −62 dBFS) or not tonal enough (purity below 0.5; noise alone scores about 0.2).

**Matched filter.** Room echo makes purity unreliable: the previous frame's reverb tail lingers into the next, pulling the purity peak late by a millisecond or more. But once a block's ids are known, its clean waveform is known exactly, so the detector cross-correlates it with the recording (via FFT). It does this in two passes:

1. *Coarse*: the envelope of the correlation with band 0 alone. Band 0 steps to a new tone every frame, so this correlation has one clean peak and weak side lobes. Echoes arrive later and weaker than the direct sound, so they can't win.
2. *Fine*: the full waveform's correlation within ±1.5 ms of the coarse peak, which finds the direct sound to the nearest sample. The full waveform alone isn't searched over the whole range: bands 1–3 hold steady tones across a block, which gives side peaks strong enough for an echo to push over the true one.

The sign of the peak gives the recording's polarity, which matters for the next step (some microphones and audio paths invert).

**Sub-sample timing.** Every tone's phase at a window's start is `2π · f · t`, where `t` is the source time at that sample (the tones are phase-coherent across frames by construction). The phase measures `t` modulo one cycle of the tone. Starting from the lowest frequency, each band's estimate is unwrapped against the running estimate, and they're combined weighted by `(f · amplitude)²`. With the window already within a sample of the boundary, there's no ambiguity left. The result is each audio frame's exact source time, typically to well under a microsecond on clean audio.

**Clean-up.** Cells decoded by two neighbouring blocks are de-duplicated, keeping the better-aligned copy. Finally, each cell's timing offset is compared with the median of the nine cells around it. Any that disagree by more than six times the typical scatter (plus 0.2 ms) are discarded. A codec's handling of the first frame after silence is the usual culprit. Genuine steps in drift survive, because the cells either side of a step agree with their own side.

## 3. Lining them up

The 16-bit audio ids are unwrapped against the video frame number shown at about the same moment, so drift up to ±32768 frames is unambiguous. Between decoded audio frames, the audio position is interpolated linearly, or extrapolated up to one and a half frames from the nearest when there's a gap. Positions before source time zero are discarded, because the audio is silent there.

**File captures.** A frame's timestamp is when it starts, so `video_position(t)` is just the frame number.

**Camera captures.** A camera frame showing frame `N` at time `t` was taken somewhere within the moment `N` was on screen. If the display shows frame `N` from `a + N/fps`, each observation bounds the display offset:

```text
t − (N + 1)/fps  ≤  a  ≤  t − N/fps
```

Intersecting these bounds over every frame read (ignoring the most extreme 2% on each side, for frames caught mid-transition) leaves a range of possible offsets. Its midpoint gives `video_position(t) = (t − a) · fps`, and its half-width is the reported *display timing* uncertainty.

When the camera and pattern rates don't divide evenly, observations land at many phases through the frame and the range shrinks to a fraction of a millisecond. When they're locked, every observation lands at the same phase and the range stays a whole frame wide. If the bounds conflict (a juddering or stalling display), the analyzer falls back to the median of `t − (N + ½)/fps` and reports half a frame of uncertainty.

## 4. The summary

Over all measured frames: mean, median, minimum, maximum and standard deviation of the drift, plus a least-squares line through drift against time. Its slope is the **skew**, reported as ms per minute and as parts per million (1 ms of drift per second = 1000 ppm). For file captures, gaps and repeats in the frame numbers are counted as dropped and repeated frames.

## Accuracy

From the test suite:

| Scenario | Result |
|----------|--------|
| Synthetic audio, any rate from 24 to 120 fps, 32–96 kHz | within 2 µs per frame |
| Synthetic audio plus noise at −20 dB | within 20 µs |
| FFV1 + PCM in `.mov` (30, 29.97, 60 fps; offsets from −40 to +500 ms) | within 0.05 ms per frame |
| H.264 + AAC in `.mp4`, same grid | within 0.5 ms per frame |
| Re-encoded at 640×360, CRF 28, AAC 96 kb/s | within 1 ms |
| `.mkv` (millisecond timestamps) | within 0.6 ms (rounding) |
| Simulated filmed screen: rotated, blurred, padded, sampled at 24.93 fps, three echoes, −12 dB | every frame within ±(display uncertainty + 1 ms); observed 40.03 ± 0.02 ms for a true 40 ms |
| Simulated room: 25 random echoes (4–120 ms), noise, −20 dB | median audio timing error 13 µs |

## Limitations

- **Locked camera rates** can't resolve video timing below ±half a frame (see the [camera guide](camera.md)).
- **Frame-rate conversion** (e.g. 30 → 25 fps) changes which source frame is shown when. Each reading is still correct for the frame shown, but the drift includes the conversion's own timing changes.
- **Very low-bandwidth audio codecs**: at 120 fps the top tones reach 8.9 kHz, which narrowband speech codecs remove. Use a lower pattern rate for those paths.
- **Audio ids repeat** every 65536 frames (18–45 minutes depending on rate). That's harmless unless audio and video are more than half that apart.
- **The recording device's own offset** is part of any camera measurement.
