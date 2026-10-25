# Measuring with a camera

The quickest way to check a real device (a TV, laptop, projector, set-top box or phone) is to play a test file on it and film the screen with a phone. The phone's microphone hears the speakers, its camera sees the picture, and `av-drift analyze` compares the two.

```sh
av-drift generate --fps 30 --duration 60 -o tv-test.mp4
# play tv-test.mp4 full screen on the device, film it, copy the recording over
av-drift analyze phone-video.mov
```

```text
Pattern  30 fps (from qr), 1280x720
Window   0.000s - 11.940s, 297 frames
Capture  camera; display timing from 237 frames caught mid-switch, known to ±1.99 ms
         (exposure measured at 33.2 ms)
Drift    mean +40.09 ms  median +40.09 ms  min +39.83 ms  max +40.30 ms  stddev 0.07 ms
         = +1.20 frames at 30 fps (positive = audio late)
Frames   297 measured, 0 unreadable, 0 without audio, 0 dropped, 0 repeated
QR       25 decoded, 272 read by fitting, 0 found but unreadable
Audio    358 frame cells decoded; tones at -22 dBFS, 108 dB above the noise
```

(Output from `scripts/stress.py`'s simulated camera: 25 fps, 1/30 s exposures, keystoned, soft and noisy, filming a 30 fps pattern whose audio is 40 ms late.)

## How camera captures differ

In a digital copy, every video frame is pixel-aligned with the original and timestamped when it starts. A camera breaks both assumptions, and the analyzer handles each:

- **The picture is at an angle, scaled, blurred and noisy.** The barcode strip can't be found where it would be in a copy, so frames are read from the **QR code**, which can be located anywhere in the image. The QR code carries everything else needed: the frame number, the exact frame rate, the audio tone plan and the test file's frame size. You don't have to tell the analyzer anything. (Scanned with an ordinary camera app, the same code opens the project's page.) Once a code has been read, the analyzer follows it from frame to frame, and the frame size tells it where the barcode strip is too, for frames whose code is too blurred or glared to read.
- **The camera's exposures blend frames.** Indoors, a phone often exposes each picture for most of a frame, so many pictures catch the screen switching from one frame to the next: every square of the QR code that differs between the two frames comes out grey, too many for an ordinary QR reader. The analyzer reads these anyway, by fitting each picture as a mix of two frames' codes, and the mix is useful: it says how far through the switch the picture was taken.
- **The camera samples the display at arbitrary moments.** A picture that shows frame N was taken at some unknown point while N was on screen. Pictures caught mid-switch pin down exactly when the display switched, so the analyzer recovers the display's timeline from them, to a millisecond or two. When few pictures are blended (a short exposure), every picture still bounds when the display switched frames, and intersecting those bounds over all the frames narrows the timeline too. Either way, the report says how tight it is as **display timing known to ±x ms**.
- **The sound crosses a room.** Echoes, background noise, a quiet recording, a speaker that can't play some of the tones, even an inverted microphone: the audio detector handles all of these (see [How it works](how-it-works.md)).

Camera mode is chosen automatically when the stamps' timing or the picture says it's a camera: frames caught at all phases of the display's frames, frames caught mid-switch, or a QR code that moves. Force it either way with `--capture camera` or `--capture file`. A capture that could be either (a camera on a stand, locked to the pattern's rate, with short exposures) is read as a copy, and the report adds a `Note` line with the camera reading.

## Getting a good measurement

**A camera frame rate that isn't a multiple of the pattern's is best.** Then the camera catches the screen at every phase, and the analyzer measures the camera's exposure time as well as the display's timing. At a rate locked to the pattern's (filming a 30 fps pattern at 30 fps, as phones do by default), the blends still time the display, but the exposure can't be measured, so the report allows for anything up to the camera's frame interval:

```text
Capture  camera; display timing from 356 frames caught mid-switch, known to ±5.02 ms
         (the camera's exposure couldn't be measured, so up to 33.3 ms is allowed for. A longer recording, or a camera rate that isn't a multiple of 30 fps, narrows this.)
```

Phones' "30 fps" is rarely exactly 30.000, so even a nominal match drifts slowly through the frame: a longer recording helps. An app that reads the camera's frames itself can pass each frame's exposure time, which phones report, and then the rate doesn't matter (see [Bring your own decoder](library.md#bring-your-own-decoder)).

Good combinations:

| Pattern | Film at |
|---------|---------|
| 30 fps (or 29.97) | 24 or 25 fps |
| 25 fps | 24 or 30 fps |
| 60 fps | 25 fps, or slow motion (120/240 fps) |

Slow-motion modes sample the screen several times per frame, which helps whatever the rates.

**Keep the QR code in shot, sharp and as big as you can.** Fill as much of the shot as you can with the screen, focus on it, and avoid reflections across the QR code. Lock exposure and focus if your camera app allows it. Frames where the code can't be read at all (heavy motion blur, glare across it) are skipped; you only need a reasonable fraction to be readable.

**Generate the pattern at a rate the screen can show.** A 60 fps pattern on a 60 Hz display is fine; a 60 fps pattern on a 50 Hz TV judders. 30 fps works almost everywhere.

**Use the device's real audio path.** Speakers, a soundbar, Bluetooth headphones held next to the microphone: whatever you want to measure. Loud isn't necessary (the analyzer reads tones 60 dB below the generated level, as long as they're above the room's noise), but the tones should be clearly audible over it. The tones are between about 600 Hz and 2.7 kHz at 30 fps (up to 4.8 kHz at 60 fps), which small speakers and phone microphones reproduce well.

**Record for at least 10 seconds.** That gives hundreds of frames for the display-timing estimate, enough phase drift to measure the exposure, and enough audio to see skew.

## What the number includes

The measurement is end to end: from the moment the device presents a frame to the moment you could see and hear it. That includes:

- the device's video processing and panel latency, and its audio output latency (usually what you're after);
- **sound travel**: about 2.9 ms per metre from speaker to microphone. Subtract it if it matters;
- **the recording device's own A/V offset.** Phones usually record sound and picture within a few milliseconds of each other, but not always. For absolute numbers, check your phone against a reference (a display you trust), or use one camera throughout and compare devices relative to each other.

Camera timestamps are taken to mark the start of each picture's exposure, of its top row, as phones record them; the analyzer allows for a rolling shutter by the readout time the camera reports, if it's given one, and otherwise from the way the blend changes down the QR code. A recorder that timestamps the middle of the exposure instead shifts the result by half the exposure time.

For context, ITU-R BT.1359 puts the threshold at which viewers notice lip-sync errors at roughly 45 ms of audio lead (audio early, negative here) and 125 ms of audio lag (positive here).

## Troubleshooting

| Symptom | Likely cause |
|---------|--------------|
| `this capture is of a test file in an older format` | The test file came from av-drift 0.3 or earlier. Generate it again with this version and film it again. |
| `could not determine the test pattern's frame rate` | No QR code was readable. Get closer, refocus, reduce glare, or check the file is an av-drift-detector test video. You can pass `--fps`, but readable QR codes are still needed for frame numbers. |
| `Warning  only N of M frames could be measured` | The line says why: no test tones in the audio (is it the test file's sound?), or QR codes found but unreadable (blur, glare, too small). |
| `no frames could be measured` | No decodable audio near the readable frames. Check the recording has sound, and that it's the test file's audio, not silence. |
| `display timing known to ±16 ms` | Timed from frame numbers alone, with the camera's rate locked to the pattern's and no frames caught mid-switch (a short exposure). See the table above. |
| `display judder at N frames` | The device showed those frames noticeably early or late: a real finding about the device (or a busy player). |
| Large `stddev` | Real jitter in the device, or an unstable recording. Check `--frames` to see where it changes. |
| Drift changes steadily (large skew) | The device's audio and video clocks differ. This is a real finding, not a measurement error. |
