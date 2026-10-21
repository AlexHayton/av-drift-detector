# Development

## The repository

A monorepo: one Rust workspace, a pnpm workspace and a uv workspace, sharing one version number.

| Path | Contents |
|------|----------|
| `crates/av-drift-detector/` | the library and the `av-drift` CLI (published to crates.io) |
| `bindings/node/` | the Node.js package (napi-rs): a Rust crate and the npm package `av-drift-detector` |
| `bindings/python/` | the Python package (PyO3 + maturin): a Rust crate and the PyPI package `av-drift-detector` |
| `docs/` | the guides |
| `scripts/` | version bumps and release tags (`version.py`, standard-library Python) |
| `Cargo.toml` | the Rust workspace: members, shared metadata, profiles |
| `package.json`, `pnpm-workspace.yaml` | the pnpm workspace |
| `pyproject.toml` | the uv workspace |

Only the crate in `crates/` is published to crates.io; the binding crates have `publish = false`.

Inside the library (`crates/av-drift-detector/src`):

| Path | Contents |
|------|----------|
| `timing.rs` | `FrameRate` (exact fractions) and time parsing |
| `pattern/` | frame rendering and stamp reading: `barcode`, `qr`, `timecode`, `layout`, `canvas` |
| `audio/` | the tone plan (`code`), synthesis (`synth`), and detection (`detect`) |
| `media/` | capture types (`MediaScan`, `AudioTrackBuilder`), and FFmpeg encoding and decoding (`ffmpeg` feature) |
| `analyze.rs` | matching video to audio, camera timing, outlier rejection |
| `report.rs` | report types and text/CSV output |
| `main.rs` | the CLI |

Only `media::{decode, encode}`, `analyze` and `generate` use FFmpeg, behind the `ffmpeg` feature.

## Building and testing

You need Rust, FFmpeg's development libraries (see [Getting started](getting-started.md#install)), and the `ffmpeg` command for the end-to-end tests. The bindings also need Node.js 20+ with [pnpm](https://pnpm.io), and [uv](https://docs.astral.sh/uv/).

**Rust.** A plain `cargo` command covers the library and the CLI; `--workspace` adds the bindings' crates.

```sh
cargo build
cargo test                    # unit, end-to-end and doc tests
cargo test -p av-drift-detector --lib --no-default-features --features qr-decode   # without FFmpeg
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all --check
```

**Node.js**, from the repository root:

```sh
pnpm install
pnpm build                    # bindings/node: the addon, index.js and index.d.ts
pnpm test                     # node --test
```

`napi build` regenerates `index.js` and `index.d.ts` from the Rust code; commit them when they change (CI checks).

**Python**, from the repository root:

```sh
uv sync                       # builds the extension with maturin and installs it, with pytest
uv run pytest bindings/python
```

uv rebuilds the extension when the Rust code changes.

The test profile is optimised (`opt-level = 3`), because the signal-processing tests are far too slow unoptimised.

| Tests | What they cover |
|-------|-----------------|
| unit tests in `crates/av-drift-detector/src/` | frame-rate arithmetic, barcode and CRC, timecode (including drop-frame), QR payloads, layout, the tone plan, audio detection on clean, noisy, echoey and inverted audio, outlier rejection, and camera timing recovery on synthetic captures |
| `crates/av-drift-detector/tests/roundtrip.rs` | generate → (re-encode with the `ffmpeg` CLI) → analyze, checked against the known answer: codecs and containers, frame rates, offsets, analysis windows, rescaling, a pipeline audio shift, a dropped frame, blended frames, and a simulated filmed screen |
| doc tests | the examples in the crate docs, `README.md` and `docs/library.md` |
| `bindings/node/test/` | generate → analyze through Node, progress, errors, and the in-memory functions |
| `bindings/python/tests/` | the same through Python, including a progress callback that raises |
| `crates/av-drift-detector/tests/ffmpeg_cli.rs` | the `ffmpeg-cli` backend against the linked one: files written by either, codecs, containers, analysis windows and a filmed screen measure the same (run with `--features ffmpeg-cli`) |

The `ffmpeg`-based tests skip themselves if the CLI isn't installed.

## Continuous integration

`.github/workflows/ci.yml` runs on every push to `main` and on pull requests:

- **Lint**: rustfmt, clippy with warnings as errors (every crate with all features, and the library with no default features), the library's tests without FFmpeg, rustdoc with warnings as errors, a check that every package has the same version, and the release script's tests (`python3 -m unittest discover -s scripts`).
- **Test**: build, the Rust test suite, the `ffmpeg-cli` parity tests, and a CLI smoke test, on Ubuntu 24.04 (FFmpeg 6.1 from apt) and macOS (the latest FFmpeg from Homebrew).
- **Node.js package** and **Python package**: build and test each binding on Ubuntu, macOS and Windows (FFmpeg from Chocolatey), and check the committed `index.js` and `index.d.ts` are up to date.
- **MSRV**: `cargo check` with Rust 1.88.

FFmpeg is installed by the local action `.github/actions/setup-ffmpeg`. `.github/workflows/packages.yml` builds the npm and PyPI packages for every platform; run it by hand from the Actions tab to try a build. `.github/workflows/release.yml` runs the CI jobs and the package builds again for each version tag before publishing (see [Releasing](#releasing)).

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

The media aren't in the published crate, which only packages `crates/av-drift-detector` (plus the README and licence). On crates.io, the README's relative links and images resolve against the `repository` URL, so they're served from GitHub.

## Versioning

Every package built from this repository shares one version number: the Rust crate, the npm package and the PyPI package. Versions follow [Semantic Versioning](https://semver.org/), limited to the forms that crates.io, npm and PyPI all accept and order the same way:

| | crates.io and npm | PyPI |
|-|-------------------|------|
| Release | `1.2.3` | `1.2.3` |
| Pre-release | `1.2.3-alpha.1`, `1.2.3-beta.1`, `1.2.3-rc.1` | `1.2.3a1`, `1.2.3b1`, `1.2.3rc1` |

`scripts/version.py` keeps them in step. It finds the manifests among the files git knows about:

- every `Cargo.toml` with a `[package]` version, except crates with `publish = false`;
- every `package.json` with a version, and the `package-lock.json` beside it;
- every `pyproject.toml` with a fixed `[project]` version. One with `dynamic = ["version"]` (maturin reads it from `Cargo.toml`) is left alone.

It also updates `Cargo.lock`, `uv.lock`, the version check in the loader napi-rs generates (`bindings/node/index.js`), version requirements between this repository's own packages, and dependency snippets such as `av-drift-detector = { version = "0.1" }` in the docs. Before committing, it asks `cargo` and `uv` (if they're installed) whether the lockfiles are up to date, and undoes the bump if not. A new binding's manifest is picked up as soon as it's added to git. CI fails if any two disagree.

## Releasing

1. As changes are merged, describe them under `## [Unreleased]` in [CHANGELOG.md](../CHANGELOG.md).
2. On an up-to-date `main`, run:

   ```sh
   scripts/version.py bump minor --dry-run   # see what would change
   scripts/version.py bump minor --push      # or major, patch, or an exact version such as 1.0.0-rc.1
   ```

   This sets the new version everywhere and turns the Unreleased notes into a dated section with a compare link. It commits `Release vX.Y.Z`, tags `vX.Y.Z`, and pushes both. Uncommitted edits to `CHANGELOG.md` are included in the commit; any other uncommitted change stops it. Without `--push` it stops after tagging, so you can look before running the `git push` it prints.
3. The tag starts the [Release workflow](../.github/workflows/release.yml). It checks that the tag is on `main`, that it matches every manifest, and that the changelog has notes for it. Then it runs the full CI suite and builds the packages ([packages.yml](../.github/workflows/packages.yml)), and publishes:
   - the crate to **crates.io**;
   - the Node.js package to **npm**: one platform package per binary (`av-drift-detector-darwin-arm64`, ...), then `av-drift-detector`, which lists them as optional dependencies so npm installs only the one that fits. `napi pre-publish` refuses to publish if any platform's binary is missing. Pre-releases get the `next` tag;
   - the Python wheels and sdist to **PyPI**.

   Each step skips a version that's already published, so a failed release can be re-run. Finally it creates the GitHub release from that version's changelog notes. Versions with `-alpha`, `-beta` or `-rc` are marked as pre-releases.

`patch`, `minor` and `major` follow npm's rules, so bumping `1.0.0-rc.2` with `patch` gives `1.0.0`. Other commands: `scripts/version.py show` prints the version, `check` verifies the manifests agree, and `notes 0.1.0` prints a version's changelog section.

### Setting up the registries

Each registry needs setting up once before the workflow can publish to it. Create the GitHub environments `crates-io`, `npm` and `pypi` (**Settings → Environments**), optionally with required reviewers.

**crates.io.** It only allows [trusted publishing](https://crates.io/docs/trusted-publishing) for a crate that already exists, which is why 0.1.0 was published by hand. On crates.io, open the crate's **Settings → Trusted Publishing** and add GitHub with owner `AlexHayton`, repository `av-drift-detector`, workflow `release.yml` and environment `crates-io`. Then delete the API token used for 0.1.0.

**npm.** Create a granular access token that can publish new packages, and save it as the secret `NPM_TOKEN` in the `npm` environment. The first release creates `av-drift-detector` and its five platform packages, each with provenance. After that you can switch each package to [trusted publishing](https://docs.npmjs.com/trusted-publishers) (workflow `release.yml`, environment `npm`) and delete the token.

**PyPI.** Add a [pending trusted publisher](https://docs.pypi.org/trusted-publishers/creating-a-project-through-oidc/) for the project `av-drift-detector`: owner `AlexHayton`, repository `av-drift-detector`, workflow `release.yml`, environment `pypi`. No token is needed; the first release creates the project.

To try the package builds without releasing, run the **Packages** workflow from the Actions tab and download its artifacts.
