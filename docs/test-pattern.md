# Test pattern specification

This page describes exactly what `av-drift generate` writes, so the pattern can be reproduced or decoded by other tools. Everything below is format version **1** for the barcode and **3** for the QR payload.

<p align="center"><img src="media/anatomy.svg" width="800" alt="Anatomy of a test-pattern frame"></p>

## Frame numbering and timing

Frame `N` (starting from 0) is presented at `N / fps` seconds, where `fps = num/den` is an exact fraction (e.g. `30000/1001`). Video and audio both start at time zero.

## Video

Frames are 8-bit YUV 4:2:0, limited range, tagged BT.709. The picture is grey-scale: chroma is 128 everywhere, black is luma 16, white is luma 235, and progress-bar slots are luma 96 (the dancer uses greys in between). Positions below are fractions of the frame width `W` and height `H`, so any scaled copy decodes the same way.

### Barcode strip

The strip spans `x` from 0.03 W to 0.97 W and `y` from 0.04 H, in **3 rows** of height 0.075 H (bottom edge at 0.265 H). Each row is divided into **18 equal cells**:

| Column | Content |
|--------|---------|
| 0 | white guard (always white) |
| 1–16 | 16 data bits |
| 17 | black guard (always black) |

Each cell is drawn inset by 1/12 of its smaller side, leaving a black gap that keeps codec ringing away from the cell centres.

The 48 data bits are read row by row, left to right, most significant first:

| Bits | Field | Value |
|------|-------|-------|
| 47–44 | version | `1` |
| 43–40 | frame-rate code | see below; `0` = other rate |
| 39–16 | frame number | 24 bits (0 to 16,777,215) |
| 15–8 | reserved | `0` |
| 7–0 | CRC-8 | over the 5 bytes holding bits 47–8 |

The CRC uses polynomial `0x07`, initial value `0x00` and final XOR `0x55`. The XOR means an all-black strip never validates. White is `1`.

Frame-rate codes:

| Code | Rate | Code | Rate | Code | Rate |
|------|------|------|------|------|------|
| 1 | 24000/1001 | 5 | 30 | 9 | 60 |
| 2 | 24 | 6 | 48 | 10 | 100 |
| 3 | 25 | 7 | 50 | 11 | 120000/1001 |
| 4 | 30000/1001 | 8 | 60000/1001 | 12 | 120 |

**Decoding.** Average a patch at each cell centre (radius: an eighth of the cell's smaller side, 1 to 6 pixels). Average the three white guards and the three black guards; if they differ by less than 48 levels, there's no barcode. Otherwise threshold each data cell at their midpoint, then check the CRC and the version.

### QR code

A QR code in a white square below the strip, on the right (see [Layout](#layout)); readers find it by searching the frame. Error correction level **M**, a 4-module quiet zone, square modules of a whole number of pixels, centred in the square.

The payload is a link to the project, so an ordinary phone camera that scans it opens the project's page. The stamp is the URL's fragment, which the page ignores:

```text
https://github.com/AlexHayton/av-drift-detector#AVD:3+F:00001234+R:30000/1001+A:T4X16G2K21
```

The fragment's fields are separated by `+`:

| Field | Format | Meaning |
|-------|--------|---------|
| `AVD:3` | literal | payload format version 3 |
| `F:` | 8 digits, zero-padded | frame number |
| `R:` | `num/den`, or `num` when `den` is 1 | exact frame rate |
| `A:` | `T<bands>X<tones>G<gap>K<k0>` | audio tone plan (below); version 1 always writes `T4X16G2K<k0>` |

The fragment uses only QR *alphanumeric* characters, which the encoder packs in their own compact segment after the URL. All fields are fixed-width for a given file, so the QR version never changes between frames: version 5, 37×37 modules, for every built-in rate. Readers should accept any URL before the `#`.

Earlier releases wrote version 2: alphanumeric fields separated by spaces, with no URL, error correction level Q, and the SMPTE timecode as a last field:

```text
AVD:2 F:00001234 R:30000/1001 A:T4X16G2K21 T:00:00:41.04
```

The analyzer reads both.

### Layout

Only the barcode strip's position is part of the format. Below it, between 0.31 H and 0.95 H and inside 0.03 W margins, the rest is arranged to suit the frame's shape:

- **Landscape, square and slightly tall frames** (`W ≥ 0.7 H`): the QR code on the right, a square of side `min(0.64 H, 0.42 W)` centred vertically; on the left, the text with the stage under it.
- **Tall frames** (`W < 0.7 H`, such as 9:16): the text across the top; below it, the stage and the QR code side by side, the QR code up to half the width.

The text is sized from the timecode's digit height: at most 0.105 H (but at least 20 pixels), and no wider than its column. A white border, `max(2, round(min(W, H) / 180))` pixels wide, frames the picture.

### Human-readable elements

Top to bottom: the SMPTE timecode, `F` plus the 8-digit frame number, a row of one slot per frame of the nominal rate with the current frame's slot lit, and an info line with the frame size, rate and length, such as `1920×1080 · 29.97 fps · 30 s`. The timecode and frame number are seven-segment digits made of rectangles and the info line uses a built-in 5×7 pixel font, so no font files are needed.

Timecode counts frames at the nominal rate (30 for 29.97) and wraps at 24 hours. At 30000/1001 and 60000/1001 it's drop-frame: frame numbers 0–1 (0–3 at 59.94) are skipped at the start of every minute except every tenth, and `;` separates the frames (`.` in the QR code).

### The dancer

On the stage, a stick figure in shades and a top hat dances with a cane and a striped scarf, on a light-up floor beside a boombox when there's room. His routine is 16 beats at 120 beats per minute, a beat on every half of the frame-in-second row, and loops every 8 seconds: frame `N` shows the routine at frame `N mod (8 × nominal fps)`. Like everything else, he's drawn from the frame number alone.

| Beats | Move | Physics |
|-------|------|---------|
| 1–4 | bounce and point, leaning on the cane | the scarf throughout: a chain of Verlet-integrated points under gravity, air drag and a breeze, simulated over one loop and cross-faded so the loop is seamless |
| 5–8 | step-touch, twirling the cane | |
| 9–12 | flick the hat up, glide, catch it on his head | the hat flies under gravity, launched at the speed that lands it on his head after one somersault; his knees take the catch on a damped spring |
| 13–16 | robot arms, then a star jump | each arm pose snaps in on an underdamped spring; the jump is ballistic, the landing absorbed by a spring, and the dust slows under drag |

Notes float up from the boombox (buoyancy against drag) and its speakers kick on the beat; floor tiles flash. Nothing on the stage carries data. Besides being fun to watch, smooth motion makes judder and dropped frames easy to see. He's left out when the stage is too small to draw him (under 14 pixels tall).

## Audio

Mono, 48 kHz by default. AAC at 256 kb/s, or 16-bit PCM.

### Frame cells

Audio frame `N` covers source times `[N/fps, (N+1)/fps)`. In samples, that starts at the first sample at or after the boundary: `ceil(N · rate · den / num)`. At 29.97 fps and 48 kHz, cells alternate between 1601 and 1602 samples.

### Tones

Each cell carries `id = N mod 65536` as four nibbles. Band `b` (0 to 3) carries nibble `n_b = (id >> 4b) & 15` as a single tone at harmonic

```text
k_b = k0 + 18·b + n_b          frequency f_b = k_b · fps
k0  = ceil(600 Hz / fps)
```

so each band spans 16 consecutive harmonics, with 2 unused harmonics between bands. The signal at source time `t`, with `frac = t·fps − floor(t·fps)`, is

```text
s(t) = Σ_b 0.08 · sin(2π · k_b · frac)     (silence for t < 0)
```

Every tone completes a whole number (`k_b`) of cycles per frame. So the waveform is continuous, zero at every frame boundary, and each tone's phase is `2π · f_b · t`, coherent across frames. The four tones peak at 0.32 of full scale, leaving room for the [music](#music). (Up to version 0.2.1 each tone's amplitude was 0.2. Decoding doesn't depend on the level, but analyzers before 0.3 ignore cells below −54 dBFS, which very quiet recordings of newer files can reach.)

| fps | k0 | spacing | band 0 | band 1 | band 2 | band 3 | audio ids repeat after |
|-----|----|---------|--------|--------|--------|--------|------------------------|
| 23.976 | 26 | 23.98 Hz | 623–983 Hz | 1055–1415 Hz | 1487–1846 Hz | 1918–2278 Hz | 45.6 min |
| 24 | 25 | 24.00 Hz | 600–960 Hz | 1032–1392 Hz | 1464–1824 Hz | 1896–2256 Hz | 45.5 min |
| 25 | 24 | 25.00 Hz | 600–975 Hz | 1050–1425 Hz | 1500–1875 Hz | 1950–2325 Hz | 43.7 min |
| 29.97 | 21 | 29.97 Hz | 629–1079 Hz | 1169–1618 Hz | 1708–2158 Hz | 2248–2697 Hz | 36.4 min |
| 30 | 20 | 30.00 Hz | 600–1050 Hz | 1140–1590 Hz | 1680–2130 Hz | 2220–2670 Hz | 36.4 min |
| 50 | 12 | 50.00 Hz | 600–1350 Hz | 1500–2250 Hz | 2400–3150 Hz | 3300–4050 Hz | 21.8 min |
| 59.94 | 11 | 59.94 Hz | 659–1558 Hz | 1738–2637 Hz | 2817–3716 Hz | 3896–4795 Hz | 18.2 min |
| 60 | 10 | 60.00 Hz | 600–1500 Hz | 1680–2580 Hz | 2760–3660 Hz | 3840–4740 Hz | 18.2 min |
| 120 | 5 | 120.00 Hz | 600–2400 Hz | 2760–4560 Hz | 4920–6720 Hz | 7080–8880 Hz | 9.1 min |

The highest tone must stay below 0.45 × the sample rate. Audio ids repeating after 65536 frames doesn't limit the measurable drift in practice: the analyzer resolves each audio id against the video frame number shown at the same moment, so offsets of up to ±32768 frames are unambiguous.

<p align="center"><img src="media/spectrogram.png" width="800" alt="Spectrogram of one second of 30 fps test audio"></p>

*One second at 30 fps: band 0 (bottom) steps up every frame, band 1 steps once at frame 16, bands 2 and 3 hold steady.*

### Music

Under the tones, `generate` mixes in a tune (`--no-music` or `GenerateOptions::music = false` leaves it out). It's built so that the decoder can't hear it:

- every partial is a harmonic of the frame rate, `k · fps`, with `k ≤ k0 − 2`: below the lowest code tone, with one harmonic spare;
- notes start, stop and change level only at frame boundaries, where all those partials are at phase zero, so there are no clicks.

Within any one frame, the music is a fixed sum of whole-cycle sinusoids on harmonics the code doesn't use. A DFT window aligned to the frame, the decoder's window, sees none of it. Decoding with and without the music gives the same results to well under a microsecond.

So the notes come from the harmonic series, in just intonation. With "do" on harmonic 8 (240 Hz at 30 fps, harmonic 16 at 15 fps), there's do, re, mi, sol, ti and the natural seventh; bass notes add overtones on harmonics too. At rates of about 48 to 60 fps, where harmonics only up to 8 are free, it plays a bugle version with "do" on harmonic 4. At 100 and 120 fps there's no room, and no music.

The tune is an oom-pah vaudeville number, 16 beats at 120 beats per minute, in step with the dancer and looping with him every 8 seconds. It was written for this project and is dedicated to the public domain ([CC0](https://creativecommons.org/publicdomain/zero/1.0/)).

It peaks at 0.58 of full scale, the room the tones leave below 0.9 (1 dB of headroom for lossy codecs). It sounds about as loud as the tones, about 2 LU quieter; together they measure about −15 LUFS. Lowering the tones to make room was tested by sweeping their level through AAC, MP3 and Opus, speaker distortion, room echo and noise. The codecs and distortion made no difference down to a quarter of this level. In echoey, noisy rooms the limit is the tones' signal-to-noise ratio, not the music: at 60 fps, below this level a noisy room starts losing frames.

### Injected offsets

`--audio-offset d` (`GenerateOptions::audio_delay`) writes output sample `s` with the signal for source time `s / rate − d`. Positive `d` delays the audio, which the analyzer reports as positive drift; before source time zero, the output is silent.

## Containers and codecs

| Option | Encoding |
|--------|----------|
| H.264 (default) | libx264, CRF 18, preset *fast*, GOP = nominal fps (one second), closed GOPs |
| FFV1 | lossless; `.mov` or `.mkv` |
| AAC (default) | 256 kb/s, mono |
| PCM | 16-bit little-endian, mono; `.mov` or `.mkv` |

`.mkv` stores timestamps in whole milliseconds, so frame times there carry up to ±0.5 ms of rounding. Use `.mov` or `.mp4` when sub-millisecond per-frame values matter.

## Versioning

The barcode's version nibble and the QR payload's `AVD:<n>` prefix change if the format does. Decoders should ignore stamps with versions they don't know, and audio plans (`A:`) they can't decode.
