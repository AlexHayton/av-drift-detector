#!/usr/bin/env python3
"""Render ready-made test clips at the most common frame sizes and rates.

    scripts/render_clips.py                 render every clip into clips/
    scripts/render_clips.py --only 1080     just the clips whose name contains "1080"
    scripts/render_clips.py --list          show what would be rendered

Each clip is an H.264 + AAC .mp4 made with `av-drift generate` (30 seconds by
default), named like av-drift-1920x1080-29.97fps-30s.mp4. Clips that already
exist are kept, so an interrupted run picks up where it stopped (--force
renders them again). Alongside them go manifest.json (what each file is, with
its size and SHA-256, and the av-drift version and commit that made it) and
SHA256SUMS, ready to upload together.

The av-drift command is built first with `cargo build --release`, unless
--av-drift names one. clips/ is in .gitignore.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import datetime
import hashlib
import json
import os
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# The frame sizes people most often test with, and the rates for each.
SIZES: list[tuple[str, int, int, list[str]]] = [
    # Landscape, 16:9
    ("4K UHD", 3840, 2160, ["30", "60"]),
    ("1440p", 2560, 1440, ["30", "60"]),
    ("1080p", 1920, 1080, ["23.976", "24", "25", "29.97", "30", "50", "59.94", "60"]),
    ("720p", 1280, 720, ["25", "29.97", "30", "50", "59.94", "60"]),
    ("480p", 854, 480, ["30"]),
    ("360p", 640, 360, ["30"]),
    # Cinema and ultrawide
    ("DCI 4K", 4096, 2160, ["24"]),
    ("Ultrawide 21:9", 2560, 1080, ["30"]),
    # Phones and social: portrait 9:16, 4:5 and square
    ("Portrait 1080p", 1080, 1920, ["30", "60"]),
    ("Portrait 720p", 720, 1280, ["30"]),
    ("Portrait 4:5", 1080, 1350, ["30"]),
    ("Square", 1080, 1080, ["30"]),
    # 4:3
    ("4:3 1080", 1440, 1080, ["30"]),
    ("4:3 VGA", 640, 480, ["30"]),
]


@dataclass(frozen=True)
class Clip:
    label: str
    width: int
    height: int
    fps: str
    seconds: float

    @property
    def name(self) -> str:
        seconds = f"{self.seconds:g}"
        return f"av-drift-{self.width}x{self.height}-{self.fps}fps-{seconds}s.mp4"

    @property
    def shape(self) -> str:
        if self.width == self.height:
            return "square"
        return "landscape" if self.width > self.height else "portrait"


def clips(seconds: float, only: list[str]) -> list[Clip]:
    every = [Clip(label, w, h, fps, seconds) for label, w, h, rates in SIZES for fps in rates]
    return [c for c in every if not only or any(o in c.name or o == c.label for o in only)]


def build() -> Path:
    print("Building av-drift (cargo build --release)...", flush=True)
    subprocess.run(["cargo", "build", "--release", "--bin", "av-drift"], cwd=ROOT, check=True)
    exe = ROOT / "target" / "release" / ("av-drift.exe" if os.name == "nt" else "av-drift")
    if not exe.exists():
        sys.exit(f"cargo didn't produce {exe}")
    return exe


def render(exe: Path, clip: Clip, out: Path) -> float:
    """Renders one clip (through a temporary name, so a half-written file is
    never mistaken for a finished one) and returns how long it took."""
    started = time.monotonic()
    partial = out / f".{clip.name}.partial.mp4"
    cmd = [
        str(exe), "generate",
        "--fps", clip.fps,
        "--duration", f"{clip.seconds:g}",
        "--size", f"{clip.width}x{clip.height}",
        "--output", str(partial),
    ]
    result = subprocess.run(cmd, capture_output=True, text=True)
    if result.returncode != 0:
        partial.unlink(missing_ok=True)
        raise RuntimeError(f"{clip.name}: {result.stderr.strip() or result.stdout.strip()}")
    partial.replace(out / clip.name)
    return time.monotonic() - started


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def git_commit() -> str | None:
    try:
        commit = subprocess.run(
            ["git", "rev-parse", "--short=12", "HEAD"], cwd=ROOT, capture_output=True, text=True, check=True
        ).stdout.strip()
        dirty = subprocess.run(["git", "status", "--porcelain"], cwd=ROOT, capture_output=True, text=True).stdout
        return commit + ("-dirty" if dirty.strip() else "")
    except (OSError, subprocess.CalledProcessError):
        return None


def write_manifest(exe: Path, out: Path, done: list[Clip]) -> None:
    version = subprocess.run([str(exe), "--version"], capture_output=True, text=True).stdout.strip()
    entries = []
    for clip in sorted(done, key=lambda c: (c.shape, -c.width * c.height, c.name)):
        path = out / clip.name
        entries.append({
            "file": clip.name,
            "label": clip.label,
            "width": clip.width,
            "height": clip.height,
            "shape": clip.shape,
            "fps": clip.fps,
            "seconds": clip.seconds,
            "video": "H.264",
            "audio": "AAC, 48 kHz mono",
            "bytes": path.stat().st_size,
            "sha256": sha256(path),
        })
    manifest = {
        "generator": version,
        "commit": git_commit(),
        "generated": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"),
        "clips": entries,
    }
    (out / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    (out / "SHA256SUMS").write_text("".join(f"{e['sha256']}  {e['file']}\n" for e in entries))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--out", type=Path, default=ROOT / "clips", help="output folder (default: clips/)")
    parser.add_argument("--seconds", type=float, default=30.0, help="length of each clip (default: 30)")
    parser.add_argument("--only", action="append", default=[], metavar="TEXT",
                        help="only clips whose file name contains TEXT, or whose label is TEXT (repeatable)")
    parser.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 4) // 4),
                        help="clips to render at once (default: a quarter of the CPUs; each encode is multi-threaded)")
    parser.add_argument("--force", action="store_true", help="render clips again even if they exist")
    parser.add_argument("--list", action="store_true", help="list the clips and exit")
    parser.add_argument("--av-drift", type=Path, metavar="PATH", help="use this av-drift instead of building one")
    args = parser.parse_args()

    todo = clips(args.seconds, args.only)
    if not todo:
        sys.exit("no clips match --only")
    if args.list:
        for c in todo:
            print(f"{c.name:48} {c.label} ({c.shape})")
        return 0

    exe = args.av_drift or build()
    args.out.mkdir(parents=True, exist_ok=True)
    pending = [c for c in todo if args.force or not (args.out / c.name).exists()]
    print(f"{len(todo)} clips, {len(todo) - len(pending)} already rendered, into {args.out}", flush=True)

    failed = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as pool:
        futures = {pool.submit(render, exe, c, args.out): c for c in pending}
        for n, future in enumerate(concurrent.futures.as_completed(futures), 1):
            clip = futures[future]
            try:
                took = future.result()
                size = (args.out / clip.name).stat().st_size / 1e6
                print(f"[{n}/{len(pending)}] {clip.name} ({size:.1f} MB, {took:.0f} s)", flush=True)
            except RuntimeError as e:
                failed.append(clip)
                print(f"[{n}/{len(pending)}] FAILED {e}", file=sys.stderr, flush=True)

    # The manifest lists every clip in the folder, not just this run's.
    done = [c for c in clips(args.seconds, []) if (args.out / c.name).exists()]
    write_manifest(exe, args.out, done)
    total = sum((args.out / c.name).stat().st_size for c in done) / 1e9
    print(f"Wrote manifest.json and SHA256SUMS for {len(done)} clips ({total:.2f} GB) in {args.out}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
