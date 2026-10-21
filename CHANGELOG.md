# Changelog

All notable changes are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Every package built from this repository shares one version number; see [Releasing](docs/development.md#releasing).

## [Unreleased]

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

[Unreleased]: https://github.com/AlexHayton/av-drift-detector/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/AlexHayton/av-drift-detector/releases/tag/v0.1.0
