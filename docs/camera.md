# Measuring with a camera

The quickest way to check a real device (a TV, laptop, projector, set-top box or phone) is to play a test file on it and film the screen with a phone. The phone's microphone hears the speakers, its camera sees the picture, and `av-drift analyze` compares the two.

```sh
av-drift generate --fps 30 --duration 60 -o tv-test.mp4
# play tv-test.mp4 full screen on the device, film it, copy the recording over
av-drift analyze phone-video.mov
```

```text
Pattern  30 fps (from qr), 1920x1080
Window   0.000s - 9.988s, 250 frames
Capture  camera; display timing known to ±0.60 ms
Drift    mean +40.03 ms  median +40.03 ms  min +39.99 ms  max +40.10 ms  stddev 0.02 ms
         = +1.20 frames at 30 fps (positive = audio late)
```

(Output from the test suite's filmed-screen simulation, where the true answer is +40.00 ms.)

## How camera captures differ

In a digital copy, every video frame is pixel-aligned with the original and timestamped when it starts. A camera breaks both assumptions, and the analyzer handles each:

- **The picture is at an angle, scaled, blurred and noisy.** The barcode strip can't be found, so frames are read from the **QR code**, which can be located anywhere in the image. The QR code carries everything else needed: the frame number, the exact frame rate and the audio tone plan. You don't have to tell the analyzer anything. (Scanned with an ordinary camera app, the same code opens the project's page.)
- **The camera samples the display at arbitrary moments.** A camera frame that shows pattern frame N was taken at some unknown point while N was on screen. Taken alone, that's only accurate to one frame (33 ms at 30 fps). But every observation "N was showing at time t" bounds when the display switched frames. The analyzer intersects those bounds over all the frames it sees, which recovers the display's timeline to a fraction of a frame. It reports how tight that is as **display timing known to ±x ms**.
- **The sound crosses a room.** Echoes, background noise, a quiet recording, even an inverted microphone: the audio detector handles all of these (see [How it works](how-it-works.md)).

Camera mode is chosen automatically when most frames had to be read from the QR code. Force it either way with `--capture camera` or `--capture file`.

## Getting a good measurement

**Pick a camera frame rate that isn't locked to the pattern.** This matters most. If the camera runs at exactly the pattern's rate (or a multiple), it catches every frame at the same point, and the display timing can't be narrowed below ±half a frame. The report then says so:

```text
Capture  camera; display timing known to ±16.67 ms
         (frames look locked to the pattern's rate. For sub-frame video timing, film at a rate that isn't a multiple of 30 fps, ...)
```

Good combinations:

| Pattern | Film at |
|---------|---------|
| 30 fps (or 29.97) | 24 or 25 fps |
| 25 fps | 24 or 30 fps |
| 60 fps | 25 fps, or slow motion (120/240 fps) |

Slow-motion modes sample the screen several times per frame, which helps whatever the rates. Phones' "30 fps" is rarely exactly 30.000, so even a nominal match drifts slowly through the frame, but it takes a long recording to help much.

**Keep the QR code big and sharp.** Fill as much of the shot as you can with the screen, focus on it, and avoid reflections across the QR code. Lock exposure and focus if your camera app allows it. Frames where the QR code can't be read (motion blur, a rolling-shutter tear between two frames, glare) are skipped; you only need a reasonable fraction to be readable.

**Generate the pattern at a rate the screen can show.** A 60 fps pattern on a 60 Hz display is fine; a 60 fps pattern on a 50 Hz TV judders. 30 fps works almost everywhere.

**Use the device's real audio path.** Speakers, a soundbar, Bluetooth headphones held next to the microphone: whatever you want to measure. Keep the volume comfortably above the room noise. The tones are between about 600 Hz and 2.7 kHz at 30 fps (up to 4.8 kHz at 60 fps), which small speakers and phone microphones reproduce well.

**Record for at least 10 seconds.** That gives hundreds of frames for the display-timing estimate, and enough audio to see skew.

## What the number includes

The measurement is end to end: from the moment the device presents a frame to the moment you could see and hear it. That includes:

- the device's video processing and panel latency, and its audio output latency (usually what you're after);
- **sound travel**: about 2.9 ms per metre from speaker to microphone. Subtract it if it matters;
- **the recording device's own A/V offset.** Phones usually record sound and picture within a few milliseconds of each other, but not always. For absolute numbers, check your phone against a reference (a display you trust), or use one camera throughout and compare devices relative to each other.

For context, ITU-R BT.1359 puts the threshold at which viewers notice lip-sync errors at roughly 45 ms of audio lead (audio early, negative here) and 125 ms of audio lag (positive here).

## Troubleshooting

| Symptom | Likely cause |
|---------|--------------|
| `could not determine the test pattern's frame rate` | No QR code was readable. Get closer, refocus, reduce glare, or check the file is an av-drift-detector test video. You can pass `--fps`, but readable QR codes are still needed for frame numbers. |
| `no frames could be measured` | No decodable audio near the readable frames. Check the recording has sound, and that it's the test file's audio, not silence. |
| `display timing known to ±16 ms` | The camera rate is locked to the pattern's; see the table above. |
| Large `stddev` | Real jitter in the device, or an unstable recording. Check `--frames` to see where it changes. |
| Drift changes steadily (large skew) | The device's audio and video clocks differ. This is a real finding, not a measurement error. |
