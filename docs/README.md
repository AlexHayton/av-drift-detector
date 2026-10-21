# av-drift-detector documentation

<p align="center"><img src="media/pattern.gif" width="480" alt="The av-drift-detector test pattern"></p>

| Guide | |
|-------|-|
| [Getting started](getting-started.md) | Install, make a first measurement, read the report |
| [Measuring with a camera](camera.md) | Point a phone at a screen: setup, accuracy, troubleshooting |
| [CLI reference](cli.md) | Every option, the JSON and CSV formats, exit codes |
| [Using the library](library.md) | The Rust API, with FFmpeg or with your own decoder (no FFmpeg needed) |
| [Test pattern specification](test-pattern.md) | The barcode, QR payload and audio code, bit by bit |
| [How it works](how-it-works.md) | The analysis algorithms, accuracy and limitations |
| [Development](development.md) | Building, testing, CI, versioning and releasing, regenerating the media |
| [Changelog](../CHANGELOG.md) | What changed in each version ([releases](https://github.com/AlexHayton/av-drift-detector/releases)) |

API documentation is on [docs.rs](https://docs.rs/av-drift-detector).

**Sample files:** [sample.mp4](media/sample.mp4) (6 s test video with audio) · [pattern.gif](media/pattern.gif) · [anatomy.svg](media/anatomy.svg) · [spectrogram.png](media/spectrogram.png)
