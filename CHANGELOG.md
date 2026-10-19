# Changelog

All notable changes are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Every package built from this repository shares one version number; see [Releasing](docs/development.md#releasing).

## [Unreleased]

First release.

### Added

- `av-drift generate`: test videos in which every frame shows its frame number as a barcode, a QR code (also carrying the exact frame rate, the audio code plan and the timecode) and SMPTE timecode, and every frame's worth of audio carries the same number as a 16-bit, four-tone code.
- `av-drift analyze`: frame-by-frame A/V drift of a capture, to within 0.05 ms for lossless files and 0.5 ms through H.264 and AAC, with mean, median, jitter, skew, and dropped and repeated frames.
- Camera captures: point a phone at a screen and its speakers. The QR code identifies frames, and the display's frame timing is recovered from many camera frames.
- Text, JSON and CSV reports, analysis windows (`--offset`, `--duration`), and `--max-drift` for use in CI.
- The `av_drift_detector` library, with FFmpeg-free pattern rendering, audio synthesis and audio detection.

[Unreleased]: https://github.com/AlexHayton/av-drift-detector/commits/main
