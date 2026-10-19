# Test pattern specification

This page describes exactly what `av-drift generate` writes, so the pattern can be reproduced or decoded by other tools. Everything below is format version **1** for the barcode and **2** for the QR payload.

<p align="center"><img src="media/anatomy.svg" width="800" alt="Anatomy of a test-pattern frame"></p>

## Frame numbering and timing

Frame `N` (starting from 0) is presented at `N / fps` seconds, where `fps = num/den` is an exact fraction (e.g. `30000/1001`). Video and audio both start at time zero.

## Video

Frames are 8-bit YUV 4:2:0, limited range, tagged BT.709. The picture is grey-scale: chroma is 128 everywhere, black is luma 16, white is luma 235, and progress-bar slots are luma 96. Positions below are fractions of the frame width `W` and height `H`, so any scaled copy decodes the same way.

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

A QR code in a white square below the strip on the right. The square has side `min(0.64 H, 0.42 W)`, right-aligned at 0.97 W, with its top at 0.31 H. Error correction level **Q**, a 4-module quiet zone, square modules of a whole number of pixels, centred in the square. The payload uses only QR *alphanumeric* characters:

```text
AVD:2 F:00001234 R:30000/1001 A:T4X16G2K21 T:00:00:41.04
```

| Field | Format | Meaning |
|-------|--------|---------|
| `AVD:2` | literal | payload format version 2 |
| `F:` | 8 digits, zero-padded | frame number |
| `R:` | `num/den`, or `num` when `den` is 1 | exact frame rate |
| `A:` | `T<bands>X<tones>G<gap>K<k0>` | audio tone plan (below); version 1 always writes `T4X16G2K<k0>` |
| `T:` | `HH:MM:SS:FF`, or `HH:MM:SS.FF` for drop-frame | SMPTE timecode |

All fields are fixed-width for a given file, so the QR version never changes between frames (version 4, 33×33 modules, for every built-in rate).

### Human-readable elements

On the left below the strip, top to bottom: the SMPTE timecode, `F` plus the 8-digit frame number, and a row of one slot per frame of the nominal rate, with the current frame's slot lit. They're drawn as seven-segment digits made of rectangles, so no font files are needed. A white border, `max(2, round(min(W, H) / 180))` pixels wide, frames the picture.

Timecode counts frames at the nominal rate (30 for 29.97) and wraps at 24 hours. At 30000/1001 and 60000/1001 it's drop-frame: frame numbers 0–1 (0–3 at 59.94) are skipped at the start of every minute except every tenth, and `;` separates the frames (`.` in the QR code).

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
s(t) = Σ_b 0.2 · sin(2π · k_b · frac)      (silence for t < 0)
```

Every tone completes a whole number (`k_b`) of cycles per frame. So the waveform is continuous, zero at every frame boundary, and each tone's phase is `2π · f_b · t`, coherent across frames. The four tones peak at 0.8 of full scale.

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
