# CLI reference

```text
av-drift [-v|--verbose] <COMMAND>

Commands:
  generate  Write a test video whose frames and audio both carry the frame number
  analyze   Measure A/V drift frame by frame in a capture of a test video (alias: check)
```

`-v, --verbose` shows FFmpeg's own log output, which is hidden apart from errors by default.

## Values

- **Times** (`--duration`, `--offset`, `--audio-offset`, `--max-drift`): plain seconds (`2.5`), suffixed values (`250ms`, `1.5s`), or clock notation (`01:30`, `00:01:30.250`). `--audio-offset` may be negative (`-40ms`).
- **Frame rates** (`--fps`): integers (`30`), decimals (`29.97`, `23.976`, `59.94`, `119.88`), or exact fractions (`30000/1001`). Decimals close to an NTSC rate (n × 1000/1001) map to the exact fraction; other decimals become exact thousandths (`12.5` → `25/2`). 1 to 480 fps.

## `av-drift generate`

```text
av-drift generate [OPTIONS] --output <OUTPUT>
```

| Option | Default | Description |
|--------|---------|-------------|
| `--fps <RATE>` | `30` | Frame rate of the pattern. |
| `--duration <TIME>` | `10` | Length, rounded to whole frames. |
| `--size <WxH>` | `1280x720` | Frame size; even, at least 320×180. |
| `--sample-rate <HZ>` | `48000` | Audio sample rate, 8000–192000. |
| `--video-codec <h264\|ffv1>` | `h264` | H.264 via libx264 (CRF 18, preset *fast*, closed GOPs of one second), or lossless FFV1. |
| `--audio-codec <aac\|pcm>` | `aac` | AAC at 256 kb/s, or 16-bit PCM. Audio is mono. |
| `--audio-offset <TIME>` | `0` | Deliberately delay the audio (negative: advance it). Useful for checking an analyser or a pipeline against a known answer. |
| `-o, --output <PATH>` | (required) | `.mp4`/`.m4v`, `.mov` or `.mkv`. FFV1 and PCM need `.mov` or `.mkv`. |

Examples:

```sh
av-drift generate --fps 59.94 --duration 00:02:00 -o broadcast.mp4
av-drift generate --fps 25 --size 1920x1080 --video-codec ffv1 --audio-codec pcm -o reference.mov
av-drift generate --fps 30 --audio-offset -25ms -o early-audio.mp4
```

## `av-drift analyze`

```text
av-drift analyze [OPTIONS] <INPUT>
```

| Option | Default | Description |
|--------|---------|-------------|
| `<INPUT>` | | Any file FFmpeg can read with a video and an audio stream. |
| `--offset <TIME>` | `0` | Start of the analysis window. |
| `--duration <TIME>` | to the end | Length of the analysis window. |
| `--fps <RATE>` | from the stamps | The pattern's frame rate. Only needed for custom rates when the QR code can't be read. |
| `--capture <auto\|file\|camera>` | `auto` | How the input was made. `file`: a digital copy whose frame timestamps are when each frame starts. `camera`: a camera filming a screen, or anything else that samples the screen at its own rate (a screen recording at another rate). `auto` decides from the stamps' timing and geometry ([how](how-it-works.md#3-lining-them-up)), and says when a capture could be either. See [Measuring with a camera](camera.md). |
| `--frames` | | Print a row for every frame before the summary. |
| `--json` | | Print the full report as JSON. |
| `--csv` | | Print one CSV row per frame. |
| `--max-drift <TIME>` | | Exit with status 2 if any frame's drift exceeds this, in either direction. |
| `--no-qr` | | Don't fall back to reading the QR code when a frame's barcode is unreadable. Faster on long digital captures with occasional bad frames. |

Only the window's frames are reported. Audio is read half a second either side, so frames at the edges still have audio to compare with.

### Per-frame table (`--frames`)

```text
      time     video        audio    drift_ms  note
    2.0020        60       57.647     +78.500
    2.0354        61       58.647     +78.500
```

`time` is the frame's presentation time in the capture. `video` is the frame number read from the picture. `audio` is the source position, in frames, of the sound playing at that moment. `drift_ms` is the difference. Notes flag `unreadable`, `no audio`, `N dropped`, `repeat`, and where the stamp came from when it isn't the barcode: `qr` (the QR code, decoded), `qr_blend` (the QR code, read by fitting it against the frames' codes) or `barcode_blend` (the barcode, fitted). Camera captures add a `video_pos` column: the estimated source position of the picture at that moment.

### Summary

The text summary's lines:

| Line | Meaning |
|------|---------|
| `Pattern` | The pattern's frame rate and where it came from, and the capture's frame size. |
| `Window` | The analysis window and how many frames it holds. |
| `Capture` | Camera captures: how the display's timing was found (from frames caught mid-switch, or from frame numbers), how precisely, and the camera's exposure where it could be measured. Also any display judder. |
| `Note` | The capture could be either a digital copy or a camera locked to the pattern's rate (a camera on a stand): what the other reading would be. |
| `Drift` | Mean, median, range and standard deviation of the drift, and the mean in frames. |
| `Skew` | The slope of drift against time. |
| `Frames` | How many frames were measured, and why the others weren't. |
| `QR` | Frames whose QR code was decoded, read by fitting, or found but unreadable. |
| `Audio` | Audio frames decoded and discarded, and how loud the tones were and how far above the noise. |
| `Warning` | Fewer than a fifth of the frames could be measured, and the likely reasons. |

### JSON (`--json`)

For example, `av-drift analyze sync.mp4 --json --offset 2 --duration 0.02` (a one-frame window):

```json
{
  "input": "sync.mp4",
  "fps": "30000/1001",
  "fps_source": "barcode",
  "capture": "file",
  "video_timing_uncertainty_ms": null,
  "video_timing": null,
  "exposure_ms": null,
  "max_exposure_ms": null,
  "display_judder": 0,
  "alternative": null,
  "width": 1280,
  "height": 720,
  "offset": 2.0,
  "duration": 0.02,
  "summary": {
    "frames": 1,
    "measured": 1,
    "mean_ms": 78.49999301156217,
    "median_ms": 78.49999301156217,
    "min_ms": 78.49999301156217,
    "max_ms": 78.49999301156217,
    "stddev_ms": 0.0,
    "skew_ms_per_min": null,
    "skew_ppm": null,
    "dropped_frames": 0,
    "repeated_frames": 0,
    "backward_jumps": 0,
    "no_video_stamp": 0,
    "no_audio": 0,
    "audio_cells": 30,
    "audio_outliers": 0,
    "audio_level_dbfs": -21.952082782321995,
    "audio_snr_db": 67.87691497802734,
    "qr_located_unread": 0,
    "qr_blend_frames": 0,
    "mixed_frames": 0
  },
  "frames": [
    {
      "time": 2.0020000000000002,
      "video_frame": 60,
      "video_position": 60.0,
      "stamp_source": "barcode",
      "video_blend": 0.0,
      "audio_frame": 57.64735285679634,
      "drift_ms": 78.49999301156217,
      "dropped_before": 0,
      "repeated": false,
      "status": "ok"
    }
  ]
}
```

| Field | Description |
|-------|-------------|
| `fps` | The pattern's exact frame rate as `num/den` (or an integer). |
| `fps_source` | `barcode`, `qr` or `option`. |
| `capture` | `file` or `camera` (the mode actually used). |
| `video_timing_uncertainty_ms` | Camera captures: ± how precisely the display's frame timing was recovered. `null` for files. |
| `video_timing` | Camera captures: `blend` (timed from frames that caught the display mid-switch) or `bounds` (from frame numbers alone). |
| `exposure_ms` | Camera captures timed from blends: the camera's exposure time, where the capture let it be measured. |
| `max_exposure_ms` | Camera captures timed from blends, where the exposure couldn't be measured: the longest allowed for (the camera's frame interval). |
| `display_judder` | Frames the display showed noticeably early or late. |
| `alternative` | When the capture could be either a digital copy or a camera locked to the pattern's rate: `{capture, mean_ms, video_timing_uncertainty_ms}` for the other reading. Otherwise `null`. |
| `summary.*_ms` | Statistics over frames with a drift value; `null` if there are none. |
| `summary.skew_ms_per_min`, `skew_ppm` | Least-squares slope of drift against time. |
| `summary.dropped_frames`, `repeated_frames` | From gaps and repeats in frame numbers (file captures only). |
| `summary.backward_jumps` | Places where the frame number went backwards (e.g. a looped or spliced capture). |
| `summary.no_video_stamp`, `no_audio` | Frames that couldn't be measured, and why. |
| `summary.audio_cells`, `audio_outliers` | Audio frames decoded; those discarded for disagreeing with their neighbours. |
| `summary.audio_level_dbfs`, `audio_snr_db` | The typical level of each decoded audio frame's weakest tone, and its height above the noise. |
| `summary.qr_located_unread` | Frames where a QR code was found but couldn't be read. |
| `summary.qr_blend_frames` | Frames read by fitting the QR code against the frames' codes (blended between two frames, too blurred to decode, or tracked from the frame before). |
| `summary.mixed_frames` | Frames that caught the display switching between two frames. |
| `frames[].status` | `ok`, `no_video_stamp` or `no_audio`. |
| `frames[].stamp_source` | `barcode`, `barcode_blend`, `qr` or `qr_blend` (see the per-frame table's notes). |
| `frames[].video_blend` | How far the picture had switched to the next frame, 0 to 1, where that was measured: 0 for a clean frame. |

### CSV (`--csv`)

```text
time,video_frame,video_position,stamp_source,video_blend,audio_frame,drift_ms,dropped_before,repeated,status
2.002000,60,60.000000,barcode,0.0000,57.647353,78.499993,0,false,ok
```

Empty fields mean "not available".

### Exit status

| Status | Meaning |
|--------|---------|
| 0 | Success. |
| 1 | Error: unreadable input, no video or audio stream, a capture of a test file in an older format (av-drift 0.3 or earlier), or no frames could be measured. |
| 2 | `--max-drift` was given and at least one frame exceeded it. |
