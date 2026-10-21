# Getting started

## Install

There are three ways in: the `av-drift` command (Rust), and packages for [Node.js](../bindings/node/README.md) and [Python](../bindings/python/README.md). The command links FFmpeg's libraries; the packages run the `ffmpeg` and `ffprobe` programs instead.

### Node.js or Python

```sh
npm install av-drift-detector     # Node.js 20 or later
pip install av-drift-detector     # Python 3.11 or later
```

Prebuilt for macOS (Apple silicon and Intel), Linux (x64 and arm64, glibc 2.17 or later) and Windows (x64). Reading and writing files needs FFmpeg 5.1 or later installed, with `ffmpeg` and `ffprobe` on your `PATH` (or set `AV_DRIFT_FFMPEG` and `AV_DRIFT_FFPROBE` to their paths). Writing H.264 test files needs an FFmpeg built with libx264, as the usual packages are:

| | |
|-|-|
| macOS | `brew install ffmpeg` |
| Debian / Ubuntu | `sudo apt install ffmpeg` |
| Fedora | `sudo dnf install ffmpeg` (from RPM Fusion; Fedora's own `ffmpeg-free` lacks libx264) |
| Windows | `winget install Gyan.FFmpeg` or `choco install ffmpeg` |

Functions that work on frames and audio in memory don't need FFmpeg at all.

### The `av-drift` command

`av-drift-detector` uses [`ffmpeg-next`](https://crates.io/crates/ffmpeg-next) to read and write media. That crate builds against your system's FFmpeg libraries, so you need FFmpeg's development files, `pkg-config`, and `libclang` (for `bindgen`). Any FFmpeg from 4.x to 9.x works; CI covers 6.1 (Ubuntu 24.04) and the latest Homebrew release.

**macOS**

```sh
brew install ffmpeg pkgconf
```

**Debian / Ubuntu**

```sh
sudo apt install ffmpeg pkg-config clang libclang-dev libavcodec-dev libavformat-dev \
    libavutil-dev libavfilter-dev libavdevice-dev libswscale-dev libswresample-dev
```

**Fedora**

```sh
sudo dnf install ffmpeg-free-devel pkgconf clang-devel
```

**Windows**: set `FFMPEG_DIR` to an FFmpeg build that includes the libraries and headers (for example, from vcpkg: `vcpkg install ffmpeg`). See the [`ffmpeg-next` build notes](https://github.com/zmwangx/rust-ffmpeg/wiki/Notes-on-building).

Then install the `av-drift` command (Rust 1.88 or later):

```sh
cargo install av-drift-detector
# or, from a checkout:
cargo install --path crates/av-drift-detector
```

The `ffmpeg` command itself isn't needed to generate or analyze. It's only used by the test suite and the examples below that put a file through a pipeline.

## Your first measurement

Generate a 10 second test file with the audio deliberately 40 ms late:

```console
$ av-drift generate --fps 30 --duration 10 --audio-offset 40ms -o test.mp4
Wrote test.mp4: 300 frames at 30 fps (10.000 s), 1280x720, audio offset +40.00 ms
```

Analyze it:

```console
$ av-drift analyze test.mp4
Input    test.mp4
Pattern  30 fps (from barcode), 1280x720
Window   0.000s - 9.967s, 300 frames
Drift    mean +40.00 ms  median +40.00 ms  min +40.00 ms  max +40.00 ms  stddev 0.00 ms
         = +1.20 frames at 30 fps (positive = audio late)
Skew     +0.000 ms/min (+0.0 ppm)
Frames   298 measured, 0 unreadable, 2 without audio, 0 dropped, 0 repeated
Audio    298 frame cells decoded
```

The first two frames have no audio to compare with, because the delayed audio hasn't started yet.

Now put the file through something real. For example, re-encode it the way a streaming service might:

```sh
ffmpeg -i test.mp4 -vf scale=854:480 -c:v libx264 -crf 28 -c:a aac -b:a 96k streamed.mp4
av-drift analyze streamed.mp4
```

The drift should still read +40.00 ms. If your pipeline changes it, the difference is what the pipeline does to sync.

## Reading the report

| Line | Meaning |
|------|---------|
| **Pattern** | The test pattern's frame rate, and where it was read from (`barcode`, `qr`, or `option` when given with `--fps`). |
| **Window** | The span analyzed (see `--offset` and `--duration`). |
| **Capture** | Shown for camera captures: how precisely the display's frame timing was recovered. See the [camera guide](camera.md). |
| **Drift** | Video position minus audio position at each frame, in milliseconds. **Positive means the audio is late** (you hear things after you see them). |
| **Skew** | The slope of drift against time. A non-zero skew means the audio and video clocks run at different rates, so drift grows steadily (100 ppm ≈ 6 ms per minute). |
| **Frames** | *measured*: frames with a drift value. *unreadable*: no stamp could be read (blended or corrupted frames). *without audio*: no decodable audio nearby (silence, or before delayed audio starts). *dropped* / *repeated*: gaps and repeats in the frame numbers, reported for file captures. |
| **Audio** | Audio frames decoded, plus any discarded because their timing disagreed with their neighbours. |

For every frame, add `--frames`; for machine-readable output, use `--json` or `--csv`. See the [CLI reference](cli.md).

## Choosing settings for the test file

- **Frame rate**: match what the system under test expects (`--fps 29.97`, `--fps 59.94`, `--fps 25`, ...). It's carried in the file, so analysis picks it up automatically.
- **Duration**: long enough to see what you care about. Clock skew needs minutes; a one-off offset needs seconds.
- **Size**: 1280×720 by default. The barcode and QR code scale with the frame, and analysis works on any resized copy.
- **Codecs**: H.264 + AAC in `.mp4` plays almost everywhere. For a lossless reference, use `--video-codec ffv1 --audio-codec pcm` with a `.mov` or `.mkv` output.

**Next:** [measure a device with a camera](camera.md), or [use the library](library.md).
