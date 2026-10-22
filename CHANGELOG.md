# Changelog

All notable changes are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Every package built from this repository shares one version number; see [Releasing](docs/development.md#releasing).

## [Unreleased]

### Added

- **A dancer.** A stick figure in shades and a top hat dances on every frame with a cane and a striped scarf, on a light-up floor beside a boombox: an 8-second, 16-beat routine at 120 bpm, in time with the frame-in-second ticks. It's physics-driven: a Verlet-simulated scarf, a hat thrown so that gravity lands it back on his head, spring-loaded robot arms, a ballistic star jump with a sprung landing, and notes and dust under drag. Like the rest of the frame, it's drawn from the frame number alone. Smooth motion also makes judder and dropped frames easy to see.
- **Music.** An oom-pah tune plays under the code tones, in step with the dancer. Every note is a harmonic of the frame rate below the lowest code tone and changes only at frame boundaries, so the decoder doesn't hear it: decoding with and without it gives the same results to under a microsecond. It plays at up to 60 fps (a bugle version at 48 to 60 fps). It's written for this project and dedicated to the public domain (CC0). Turn it off with `av-drift generate --no-music`, `GenerateOptions::music`, or the bindings' `music` option. In the library, it's `audio::Music` and `ToneSynth::with_music`.
- **An info line** under the frame-in-second ticks shows the frame size, rate and length, such as `1920×1080 · 29.97 fps · 30 s`. `PatternRenderer::with_total_frames` sets the length and `PatternRenderer::info` returns the line. The Node.js and Python `PatternRenderer` take a `duration`.
- `scripts/render_clips.py` renders 30-second clips at common frame sizes and rates, with a manifest and checksums, for hosting.

### Changed

- The frame is laid out for its shape. Landscape, square and slightly tall frames keep the QR code on the right, with the text and the dancer's stage on the left. Tall frames (narrower than 0.7 × their height, such as 9:16) put the text across the top, and the stage and QR code side by side below it. The timecode is smaller, at most 0.105 of the frame height. The barcode strip hasn't moved. `Layout` has new `info` and `stage` fields.
- **The code tones are 8 dB quieter** (amplitude 0.08 each, peaking at 0.32 together, from 0.2 and 0.8) to make room for the music, which sounds about as loud as them. Together they measure about −15 LUFS, with 3 dB of headroom. In a sweep through AAC, MP3 and Opus, speaker distortion, room echo and noise, this cost nothing measurable, except about 2% of frames in a noisy room at 60 fps. The decoder's loudness floor follows the tones, from −54 to −62 dBFS. Older analyzers can still read new files, but very quiet recordings of them may fall below the old floor.
- `GenerateOptions` has a new `music` field (on by default). `synthesizeAudio` and `synthesize_audio` in the bindings include the music unless told not to. `audio::synthesize` still makes the code alone.

## [0.2.1] - 2026-10-21

### Fixed

- 0.2.0 was tagged but never published: its release stopped when the Node.js and Python packages' checks found files still marked 0.1.0. This release publishes everything listed under 0.2.0.

## [0.2.0] - 2026-10-21

### Added

- **Node.js package** `av-drift-detector` (`bindings/node`, built with napi-rs): `analyze` and `generate`, plus `analyzeFrames`, `readFrame`, `PatternRenderer` and `synthesizeAudio` for frames and audio in memory. Promise-based, with TypeScript types. Prebuilt for macOS, Linux and Windows. See [its README](bindings/node/README.md).
- **Python package** `av-drift-detector` (`bindings/python`, built with PyO3 and maturin): the same functions, releasing the GIL, with reports as the dicts `av-drift analyze --json` writes. Wheels for macOS, Linux and Windows. See [its README](bindings/python/README.md).
- The `ffmpeg-cli` feature: `media::ffmpeg_cli::{analyze, generate}` read and write files by running the `ffmpeg` and `ffprobe` programs (FFmpeg 5.1 or later), linking nothing, with results that match the linked path frame for frame. The Node.js and Python packages use it, so their binaries contain no FFmpeg.
- `media::AudioTrackBuilder`, to assemble an audio track from timestamped chunks; `pattern::qr::locate`, which also reports where the QR code is and can search just a region; `LumaView::crop`; and `FromStr` for `CaptureMode`, `VideoCodec` and `AudioCodec`. `GenerateOptions`, `VideoCodec`, `AudioCodec` and `Progress` now live in `media` and don't need FFmpeg (still re-exported at the crate root).

### Changed

- The QR code is now a link: scanned with a phone's camera app, it opens this project's page, with the stamp in the URL's fragment (payload format 3: `https://github.com/AlexHayton/av-drift-detector#AVD:3+F:...`). It uses error correction level M and stays at version 5 (37×37) for every rate. The timecode is no longer in the QR code (it's still on screen). Format 2 codes from 0.1.0 files are still read, but 0.1.0 can't read the new codes, so analyse new camera captures with this release.
- **Breaking:** FFmpeg is now behind the `ffmpeg` feature, on by default. If you turn default features off, add `"ffmpeg"` to keep `analyze` and `generate`. Without it the library, including the analysis itself (`analyze_scan`), builds with no FFmpeg, for targets such as Android.
- The repository is a monorepo: the crate lives in `crates/av-drift-detector`, beside the Node.js and Python bindings (`bindings/`). One version number still covers every package.

## [0.1.0] - 2026-10-20

First release.

### Added

- `av-drift generate`: test videos in which every frame shows its frame number as a barcode, a QR code (also carrying the exact frame rate, the audio code plan and the timecode) and SMPTE timecode, and every frame's worth of audio carries the same number as a 16-bit, four-tone code.
- `av-drift analyze`: frame-by-frame A/V drift of a capture, to within 0.05 ms for lossless files and 0.5 ms through H.264 and AAC, with mean, median, jitter, skew, and dropped and repeated frames.
- Camera captures: point a phone at a screen and its speakers. The QR code identifies frames, and the display's frame timing is recovered from many camera frames.
- Text, JSON and CSV reports, analysis windows (`--offset`, `--duration`), and `--max-drift` for use in CI.
- The `av_drift_detector` library, with FFmpeg-free pattern rendering, audio synthesis and audio detection.

[Unreleased]: https://github.com/AlexHayton/av-drift-detector/compare/v0.2.1...HEAD
[0.2.1]: https://github.com/AlexHayton/av-drift-detector/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/AlexHayton/av-drift-detector/releases/tag/v0.2.0
[0.1.0]: https://github.com/AlexHayton/av-drift-detector/releases/tag/v0.1.0
