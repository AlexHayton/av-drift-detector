# av-drift-detector (Node.js)

**Measure audio/video sync to a fraction of a millisecond, frame by frame.**

Node.js bindings to [av-drift-detector](https://github.com/AlexHayton/av-drift-detector), the Rust library behind the `av-drift` command. Generate a test video whose every frame, and every frame's worth of audio, carries its own frame number. Play it through whatever you want to test (an encoder, a streaming pipeline, a TV, a video call), record the result, and measure how far the sound has drifted from the picture at every frame.

```js
import avd from 'av-drift-detector'

await avd.generate({ output: 'sync.mp4', fps: '29.97', duration: 10, audioOffset: 0.04 })
// ... play sync.mp4 through the system under test and record it as captured.mp4 ...
const report = await avd.analyze('captured.mp4')
console.log(report.summary.meanMs) // positive means the audio is late
```

## Install

```sh
npm install av-drift-detector
```

Node.js 20 or later, on macOS (Apple silicon and Intel), Linux (x64 and arm64, glibc 2.17+) or Windows (x64). TypeScript types are included.

Reading and writing files runs the `ffmpeg` and `ffprobe` programs, so install FFmpeg 5.1 or later (`brew install ffmpeg`, `sudo apt install ffmpeg`, `winget install Gyan.FFmpeg`) and keep them on your `PATH`, or set `AV_DRIFT_FFMPEG` and `AV_DRIFT_FFPROBE` to their paths. Writing H.264 needs an FFmpeg with libx264, as those packages have. The in-memory functions below don't need FFmpeg.

## Files

- **`generate(options, onProgress?)`** writes a test video and resolves to a summary. Only `output` is required; the others default to `fps: '30'`, `duration: 10` (seconds), 1280×720, `sampleRate: 48000`, `videoCodec: 'h264'`, `audioCodec: 'aac'` and `audioOffset: 0` (seconds; positive delays the audio). `onProgress({ framesDone, framesTotal })` is called about once a percent. FFV1 video and PCM audio need `.mov` or `.mkv`.
- **`analyze(path, options?)`** measures every frame of a capture. Options: `offset` and `duration` (a window, in seconds), `fps` (override the pattern's rate), `useQr`, and `capture`: `'auto'`, `'file'` (a digital copy) or `'camera'` (a phone filming a screen).

Both run on libuv's thread pool and return promises. Reports have the content of `av-drift analyze --json` ([format](https://github.com/AlexHayton/av-drift-detector/blob/main/docs/cli.md#json---json)) with camelCase names:

```js
const { meanMs, stddevMs, skewPpm } = report.summary
for (const f of report.frames) {
  if (f.status === 'ok') console.log(f.time, f.videoFrame, f.driftMs)
}
```

## Frames and audio from elsewhere

If you already have decoded frames and audio (a capture card, WebRTC, a camera), measure them without files:

```js
const frames = myFrames.map(({ time, luma }) => ({
  time,                                       // seconds
  stamp: avd.readFrame(luma, width, height),  // 8-bit luma (Y) plane
}))
const report = await avd.analyzeFrames({
  frames,
  audio: { sampleRate: 48000, startTime: t0, samples: monoFloat32Array },
  width,
  height,
})
```

Frame times and `startTime` must be on the same clock, in seconds.

- **`readFrame(luma, width, height, stride?, tryQr?)`** reads a frame's stamp (barcode, then QR code), or returns `null`.
- **`new PatternRenderer({ fps, width, height, sampleRate }).render(frameId)`** draws a test-pattern frame as a luma `Buffer`.
- **`synthesizeAudio({ count, fps, sampleRate, delay, start })`** makes the test audio as a `Float32Array`.

Async functions reject on bad arguments; `readFrame` and the renderer throw.

## More

The [repository](https://github.com/AlexHayton/av-drift-detector) has the guides: measuring with a camera, the test pattern specification, and how the analysis works. MIT licensed.
