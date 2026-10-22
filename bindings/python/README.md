# av-drift-detector (Python)

**Measure audio/video sync to a fraction of a millisecond, frame by frame.**

Python bindings to [av-drift-detector](https://github.com/AlexHayton/av-drift-detector), the Rust library behind the `av-drift` command. Generate a test video whose every frame, and every frame's worth of audio, carries its own frame number. Play it through whatever you want to test (an encoder, a streaming pipeline, a TV, a video call), record the result, and measure how far the sound has drifted from the picture at every frame.

```python
import av_drift_detector as avd

avd.generate("sync.mp4", fps="29.97", duration=10, audio_offset=0.040)
# ... play sync.mp4 through the system under test and record it as captured.mp4 ...
report = avd.analyze("captured.mp4")
print(report["summary"]["mean_ms"])  # positive means the audio is late
```

## Install

```sh
pip install av-drift-detector
```

Python 3.11 or later. Wheels cover macOS (Apple silicon and Intel), Linux (x64 and arm64, manylinux) and Windows (x64); elsewhere pip builds from source, which needs [Rust](https://rustup.rs).

Reading and writing files runs the `ffmpeg` and `ffprobe` programs, so install FFmpeg 5.1 or later (`brew install ffmpeg`, `sudo apt install ffmpeg`, `winget install Gyan.FFmpeg`) and keep them on your `PATH`, or set `AV_DRIFT_FFMPEG` and `AV_DRIFT_FFPROBE` to their paths. Writing H.264 needs an FFmpeg with libx264, as those packages have. The in-memory functions below don't need FFmpeg.

## Files

- **`generate(output, *, fps="30", duration=10.0, width=1280, height=720, sample_rate=48000, video_codec="h264", audio_codec="aac", audio_offset=0.0, music=True, on_progress=None)`** writes a test video and returns a summary. `audio_offset` (seconds) deliberately delays the audio; negative makes it early. `music` plays a tune under the tones that doesn't affect analysis. `on_progress(frames_done, frames_total)` is called about once a percent. FFV1 video and PCM audio need `.mov` or `.mkv`.
- **`analyze(path, *, offset=0.0, duration=None, fps=None, use_qr=True, capture="auto")`** measures every frame of a capture. `offset` and `duration` choose a window in seconds, and `capture` is `"auto"`, `"file"` (a digital copy) or `"camera"` (a phone filming a screen).

Reports are the dicts `av-drift analyze --json` writes ([format](https://github.com/AlexHayton/av-drift-detector/blob/main/docs/cli.md#json---json)), typed as `DriftReport`, so `json.dump(report, f)` saves the CLI's format:

```python
s = report["summary"]
print(f"{s['mean_ms']:+.2f} ms ± {s['stddev_ms']:.2f}, skew {s['skew_ppm']:+.1f} ppm")
for f in report["frames"]:
    if f["status"] == "ok":
        print(f["time"], f["video_frame"], f["drift_ms"])
```

## Frames and audio from elsewhere

If you already have decoded frames and audio (a capture card, WebRTC, a camera), measure them without files:

```python
frames = []
for time, luma in my_frames():           # 8-bit luma (Y) planes: bytes, numpy uint8, ...
    frames.append({"time": time, "stamp": avd.read_frame(luma, width, height)})

audio = {"sample_rate": 48_000, "start_time": t0, "samples": mono_float32}  # array("f"), numpy float32, ...
report = avd.analyze_frames(frames, audio, width=width, height=height)
```

Frame times and `start_time` must be on the same clock, in seconds.

- **`read_frame(luma, width, height, stride=None, *, try_qr=True)`** reads a frame's stamp (barcode, then QR code), or returns None.
- **`PatternRenderer(*, fps="30", width=1280, height=720, sample_rate=48000, duration=None).render(frame_id)`** draws a test-pattern frame as luma bytes. With `duration` (seconds), the info line shows the file's length.
- **`synthesize_audio(count, *, fps="30", sample_rate=48000, delay=0.0, start=0, music=True)`** makes the test audio as an `array("f")`, with the music unless `music` is false.

Errors raise `AvDriftError`. Slow calls release the GIL, so they run in parallel from threads.

## More

The [repository](https://github.com/AlexHayton/av-drift-detector) has the guides: measuring with a camera, the test pattern specification, and how the analysis works. MIT licensed.
