# Development

## Building and testing

You need FFmpeg's development libraries (see [Getting started](getting-started.md#install)), and the `ffmpeg` command for the end-to-end tests.

```sh
cargo build
cargo test                    # unit, end-to-end and doc tests
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all --check
```

The test profile is optimised (`opt-level = 3`), because the signal-processing tests are far too slow unoptimised.

| Tests | What they cover |
|-------|-----------------|
| unit tests in `src/` | frame-rate arithmetic, barcode and CRC, timecode (including drop-frame), QR payloads, layout, the tone plan, audio detection on clean, noisy, echoey and inverted audio, outlier rejection, and camera timing recovery on synthetic captures |
| `tests/roundtrip.rs` | generate → (re-encode with the `ffmpeg` CLI) → analyze, checked against the known answer: codecs and containers, frame rates, offsets, analysis windows, rescaling, a pipeline audio shift, a dropped frame, blended frames, and a simulated filmed screen |
| doc tests | the examples in the crate docs, `README.md` and `docs/library.md` |

The `ffmpeg`-based tests skip themselves if the CLI isn't installed.

## Layout

| Path | Contents |
|------|----------|
| `src/timing.rs` | `FrameRate` (exact fractions) and time parsing |
| `src/pattern/` | frame rendering and stamp reading: `barcode`, `qr`, `timecode`, `layout`, `canvas` |
| `src/audio/` | the tone plan (`code`), synthesis (`synth`), and detection (`detect`) |
| `src/media/` | FFmpeg encoding and decoding |
| `src/analyze.rs` | matching video to audio, camera timing, outlier rejection |
| `src/report.rs` | report types and text/CSV output |
| `src/main.rs` | the CLI |
| `scripts/version.py` | version bumps and release tags (tests in `scripts/test_version.py`) |

`pattern` and `audio` don't use FFmpeg.

## Continuous integration

`.github/workflows/ci.yml` runs on every push to `main` and on pull requests:

- **Lint**: rustfmt, clippy with warnings as errors (all features, and the library with no default features), rustdoc with warnings as errors, a check that every package has the same version, and the release script's tests (`python3 -m unittest discover -s scripts`).
- **Test**: build, the full test suite, and a CLI smoke test, on Ubuntu 24.04 (FFmpeg 6.1 from apt) and macOS (the latest FFmpeg from Homebrew).
- **MSRV**: `cargo check` with Rust 1.88.

FFmpeg is installed by the local action `.github/actions/setup-ffmpeg`. `.github/workflows/release.yml` runs the same jobs again for each version tag before publishing it (see [Releasing](#releasing)).

## Regenerating the README media

From the repository root, with a release build:

```sh
cargo build --release
AVD=target/release/av-drift

# Sample video (6 s, 30 fps, H.264 + AAC)
$AVD generate --fps 30 --duration 6 -o docs/media/sample.mp4

# Animated GIF: 25 fps so GIF frame delays (in 1/100 s) are exact; rendered at GIF size, lossless
$AVD generate --fps 25 --duration 2.4 --size 640x360 --video-codec ffv1 --audio-codec pcm -o /tmp/gif-src.mkv
ffmpeg -i /tmp/gif-src.mkv \
  -vf "split[a][b];[a]palettegen=max_colors=8:reserve_transparent=0[p];[b][p]paletteuse=dither=none" \
  -loop 0 docs/media/pattern.gif

# Spectrogram of the first second of audio
ffmpeg -t 1 -i docs/media/sample.mp4 \
  -lavfi "showspectrumpic=s=900x420:legend=1:fscale=lin:start=500:stop=2800:color=viridis:gain=4" \
  -frames:v 1 -update 1 docs/media/spectrogram.png
```

`docs/media/anatomy.svg` is a frame of the sample (at 1.4 s, frame 42) with labelled outlines. The frame is embedded as a base64 PNG, so the SVG renders on its own anywhere.

The media are excluded from the published crate (`exclude` in `Cargo.toml`). On crates.io, the README's relative links and images resolve against the `repository` URL in `Cargo.toml`, so they're served from GitHub.

## Versioning

Every package built from this repository shares one version number: the Rust crate now, and npm and PyPI packages when there are bindings. Versions follow [Semantic Versioning](https://semver.org/), limited to the forms that crates.io, npm and PyPI all accept and order the same way:

| | crates.io and npm | PyPI |
|-|-------------------|------|
| Release | `1.2.3` | `1.2.3` |
| Pre-release | `1.2.3-alpha.1`, `1.2.3-beta.1`, `1.2.3-rc.1` | `1.2.3a1`, `1.2.3b1`, `1.2.3rc1` |

`scripts/version.py` keeps them in step. It finds the manifests among the files git knows about:

- every `Cargo.toml` with a `[package]` version, except crates with `publish = false`;
- every `package.json` with a version, and the `package-lock.json` beside it;
- every `pyproject.toml` with a fixed `[project]` version. One with `dynamic = ["version"]` (maturin reads it from `Cargo.toml`) is left alone.

It also updates `Cargo.lock`, version requirements between this repository's own packages, and dependency snippets such as `av-drift-detector = { version = "0.1" }` in the docs. A new binding's manifest is picked up as soon as it's added to git. CI fails if any two disagree.

## Releasing

1. As changes are merged, describe them under `## [Unreleased]` in [CHANGELOG.md](../CHANGELOG.md).
2. On an up-to-date `main`, run:

   ```sh
   scripts/version.py bump minor --dry-run   # see what would change
   scripts/version.py bump minor --push      # or major, patch, or an exact version such as 1.0.0-rc.1
   ```

   This sets the new version everywhere and turns the Unreleased notes into a dated section with a compare link. It commits `Release vX.Y.Z`, tags `vX.Y.Z`, and pushes both. Uncommitted edits to `CHANGELOG.md` are included in the commit; any other uncommitted change stops it. Without `--push` it stops after tagging, so you can look before running the `git push` it prints.
3. The tag starts the [Release workflow](../.github/workflows/release.yml). It checks that the tag is on `main`, that it matches every manifest, and that the changelog has notes for it. Then it runs the full CI suite, publishes to crates.io, and creates the GitHub release from that version's changelog notes. Versions with `-alpha`, `-beta` or `-rc` are marked as pre-releases.

`patch`, `minor` and `major` follow npm's rules, so bumping `1.0.0-rc.2` with `patch` gives `1.0.0`. Other commands: `scripts/version.py show` prints the version, `check` verifies the manifests agree, and `notes 0.1.0` prints a version's changelog section.

### The first release

crates.io only allows [trusted publishing](https://crates.io/docs/trusted-publishing) from GitHub Actions for a crate that already exists, so 0.1.0 is published by hand:

1. Sign in to [crates.io](https://crates.io) with GitHub, verify your email address, and create an API token with the `publish-new` scope. Run `cargo login` and paste it.
2. `scripts/version.py bump 0.1.0`. The version is already 0.1.0, so this only dates the changelog, commits and tags.
3. `cargo publish`.
4. `git push --atomic origin main v0.1.0`. The Release workflow finds 0.1.0 already on crates.io, so it skips publishing and creates the GitHub release.
5. On crates.io, open the crate's **Settings → Trusted Publishing** and add GitHub with owner `AlexHayton`, repository `av-drift-detector`, workflow `release.yml` and environment `crates-io`. Then delete the API token: later releases publish from the workflow without one.
