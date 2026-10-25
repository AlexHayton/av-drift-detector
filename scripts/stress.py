#!/usr/bin/env python3
"""Stress-test the analyzer: damage a test clip in many ways and check each result.

Generates a 12-second 720p test clip with the audio 40 ms late, then makes a
copy of it per case with FFmpeg: digital copies rotated, keystoned, blurred,
shrunk, compressed or placed in a bigger picture; simulated cameras whose
exposures blend frames; and audio that's delayed, quiet, filtered, echoey,
noisy, denoised, re-encoded or clock-skewed. Each copy is analysed with
``av-drift analyze --json`` and checked against the known answer.

    cargo build --release
    python3 scripts/stress.py                 # every case
    python3 scripts/stress.py camera audio.   # cases whose names start with these

Needs FFmpeg with libx264, libopus and libmp3lame on PATH. Takes a minute or
two on a laptop; the clips go in a temporary directory (``--keep DIR`` keeps them).
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TRUTH = 40.0  # ms: the base clip's audio delay


@dataclass
class Case:
    name: str
    video: str | None = None  # an FFmpeg filter graph, or None to copy the video
    audio: str | None = None  # an FFmpeg audio filter, or None to copy the audio
    truth: float | None = TRUTH  # ms; None: report only
    within: float = 0.5  # ms either side of the truth
    # Camera cases with an unmeasurable exposure pass if within their own uncertainty.
    honest: bool = False
    complex_video: bool = False  # `video` is a -filter_complex graph with [v] out
    noise: float = 0.0  # pink noise amplitude to mix into the audio
    args: list[str] = field(default_factory=list)
    container: str = "mov"


def camera(fps: int, exposure: int, phase: int, after: str = "") -> str:
    """A camera: oversample to 300 fps, average ``exposure`` samples (1/300 s
    each), keep every 300/fps-th from ``phase``, timestamped at the start of
    the exposure as phones do. The first 0.1 s goes, so that no timestamp is
    negative (a muxer would shift the whole track)."""
    graph = (
        f"fps=300,tmix=frames={exposure},setpts=PTS-{exposure - 1},trim=start=0.1,"
        f"select='not(mod(n-{phase}\\,{300 // fps}))'"
    )
    return graph + ("," + after if after else "")


def even(graph: str) -> str:
    return graph + ",scale=trunc(iw/2)*2:trunc(ih/2)*2"


def rotate(degrees: float, after: str = "") -> str:
    a = f"{degrees}*PI/180"
    return even(f"rotate={a}:ow=rotw({a}):oh=roth({a}):fillcolor=gray" + ("," + after if after else ""))


KEYSTONE = "perspective=x0=40:y0=30:x1=W-10:y1=0:x2=10:y2=H-5:x3=W-60:y3=H-45:sense=destination"
SCENE = (
    "[0:v]scale=760:-2,rotate=5*PI/180:fillcolor=black:ow=rotw(5*PI/180):oh=roth(5*PI/180)[p];"
    "testsrc2=s=1920x1080:r=30:d=12[bg];[bg][p]overlay=x=900:y=250:shortest=1"
)
HANDHELD = f"{KEYSTONE},gblur=sigma=1.2,noise=alls=12:allf=t"

CASES = [
    # Digital copies: frames timestamped when they start, however mangled.
    Case("copy.plain", "null"),
    Case("copy.rotate-10", rotate(10)),
    Case("copy.rotate-30", rotate(30)),
    Case("copy.rotate-90", "transpose=1"),
    Case("copy.keystone", KEYSTONE),
    Case("copy.keystone-blur-noise", f"{KEYSTONE},gblur=sigma=2,noise=alls=20:allf=t"),
    Case("copy.blur-5", "gblur=sigma=5"),
    Case("copy.noise", "noise=alls=30:allf=t+u"),
    Case("copy.washed-out", "eq=contrast=0.35:brightness=0.3"),
    Case("copy.glare", "geq=lum='clip(p(X\\,Y)*0.55+115*X/W\\,0\\,255)':cb='cb(X\\,Y)':cr='cr(X\\,Y)'"),
    Case("copy.small-320", "scale=320:180"),
    Case("copy.rotate-10-small-360", rotate(10, "scale=360:-2")),
    Case("copy.crf-45", "null", args=["-crf", "45"]),
    Case("copy.in-a-scene", SCENE, complex_video=True),
    Case("copy.in-4k", "scale=1100:-2,pad=3840:2160:2400:900:gray"),
    # Cameras at another rate: blends time the display, and measure the exposure.
    Case("camera.25fps-33ms", camera(25, 10, 0), within=1.0),
    # In 1/300 s steps, these two catch only one or two distinct mixes (real
    # cameras catch every phase), so the exposure can't be measured: they're
    # judged against their own stated uncertainty.
    Case("camera.25fps-13ms-handheld", camera(25, 4, 0, HANDHELD), honest=True),
    Case("camera.60fps-17ms", camera(60, 5, 0), honest=True),
    # Cameras locked to the pattern's rate: honest about what they can't know.
    Case("camera.30fps-33ms-phase3", camera(30, 10, 3), honest=True),
    Case("camera.30fps-33ms-phase7", camera(30, 10, 7), honest=True),
    Case("camera.30fps-33ms-handheld", camera(30, 10, 2, HANDHELD), honest=True),
    # Audio.
    Case("audio.delay+100ms", audio="adelay=delays=100:all=1", truth=TRUTH + 100),
    Case("audio.early-100ms", audio="atrim=start=0.1,asetpts=PTS-STARTPTS", truth=TRUTH - 100),
    Case("audio.quiet-40dB", audio="volume=-40dB"),
    Case("audio.quiet-60dB", audio="volume=-60dB"),
    # Filters delay sound, by their group delay: a little leeway.
    Case("audio.small-speaker", audio="highpass=f=750:poles=2,highpass=f=750:poles=2,lowpass=f=2000", within=0.6),
    Case("audio.notch-band-2", audio="equalizer=f=1680:t=q:w=6:g=-30,equalizer=f=1680:t=q:w=6:g=-30", within=0.6),
    Case("audio.echo", audio="aecho=0.8:0.9:23|61|127:0.6|0.45|0.3"),
    Case("audio.pink-noise", audio="anull", noise=0.06),
    # afftdn's own processing delay is 25 ms: the analyzer should see it.
    Case("audio.denoise", audio="afftdn=nr=30:nf=-50:tn=1", truth=TRUTH + 25),
    Case("audio.agc", audio="dynaudnorm=f=100:g=5:p=0.9"),
    Case("audio.skew+500ppm", audio="asetrate=48024,aresample=48000", truth=None),
    Case("audio.aac-32k", audio="anull", args=["-c:a", "aac", "-b:a", "32k"]),
    Case("audio.opus-16k", audio="anull", args=["-c:a", "libopus", "-b:a", "16k"], container="mkv"),
    Case("audio.mp3-48k", audio="anull", args=["-c:a", "libmp3lame", "-b:a", "48k"], container="mkv"),
    Case("audio.phone", audio="volume=-38dB,highpass=f=700,lowpass=f=2600,aecho=0.8:0.8:19|53:0.5|0.3", noise=0.0004, within=0.6),
]


def make(case: Case, base: Path, out_dir: Path) -> Path:
    out = out_dir / f"{case.name}.{case.container}"
    cmd = ["ffmpeg", "-v", "error", "-y", "-i", str(base)]
    audio = case.audio
    if case.noise:
        cmd += [
            "-filter_complex",
            f"[0:a]{audio or 'anull'}[s];anoisesrc=a={case.noise}:c=pink:d=13:r=48000[n];"
            "[s][n]amix=inputs=2:normalize=0:duration=first[a]",
            "-map", "0:v", "-map", "[a]",
        ]
        audio = None
    if case.complex_video:
        cmd += ["-filter_complex", f"{case.video}[v]", "-map", "[v]", "-map", "0:a"]
    elif case.video:
        cmd += ["-vf", case.video, "-fps_mode", "vfr"]
    if audio:
        cmd += ["-af", audio]
    cmd += ["-c:v", "copy"] if case.video is None else ["-c:v", "libx264", "-preset", "veryfast"]
    if case.video is not None and "-crf" not in case.args:
        cmd += ["-crf", "16"]
    if "-c:a" not in case.args:
        cmd += ["-c:a", "pcm_s16le" if (case.audio or case.noise) else "copy"]
    cmd += [*case.args, str(out)]
    subprocess.run(cmd, check=True)
    return out


def analyze(binary: Path, path: Path) -> dict:
    r = subprocess.run([str(binary), "analyze", "--json", str(path)], capture_output=True, text=True)
    if r.returncode not in (0, 2):
        return {"error": (r.stderr or r.stdout).strip().splitlines()[-1]}
    return json.loads(r.stdout)


def verdict(case: Case, report: dict) -> tuple[bool | None, str]:
    """Whether the case passed, and the error to show."""
    if "error" in report:
        return False, ""
    mean = report["summary"]["mean_ms"]
    if case.truth is None or mean is None:
        return (None if mean is not None else False), ""
    error = mean - case.truth
    if case.honest:
        allowed = (report.get("video_timing_uncertainty_ms") or 0.0) + 0.5
    else:
        allowed = case.within
    return abs(error) <= allowed, f"{error:+.2f}"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("prefixes", nargs="*", help="run only cases whose names start with these")
    parser.add_argument("--bin", type=Path, default=ROOT / "target" / "release" / "av-drift")
    parser.add_argument("--keep", type=Path, help="write the clips here and keep them")
    parser.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) // 3))
    args = parser.parse_args()
    if not args.bin.exists():
        print(f"no analyzer at {args.bin}: run `cargo build --release` first", file=sys.stderr)
        return 1
    cases = [c for c in CASES if not args.prefixes or any(c.name.startswith(p) for p in args.prefixes)]

    with tempfile.TemporaryDirectory() as tmp:
        out_dir = args.keep or Path(tmp)
        out_dir.mkdir(parents=True, exist_ok=True)
        base = out_dir / "base.mov"
        subprocess.run(
            [str(args.bin), "generate", "--fps", "30", "--duration", "12", "--size", "1280x720",
             "--audio-codec", "pcm", "--audio-offset", f"{TRUTH}ms", "-o", str(base)],
            check=True, capture_output=True,
        )

        def run(case: Case) -> tuple[Case, dict]:
            try:
                return case, analyze(args.bin, make(case, base, out_dir))
            except subprocess.CalledProcessError as e:
                return case, {"error": f"ffmpeg failed: {e}"}

        with ThreadPoolExecutor(args.jobs) as pool:
            results = list(pool.map(run, cases))

    header = f"{'case':32s} {'mode':6s} {'timing':6s} {'meas':>5s} {'unread':>6s} {'cells':>5s} {'mean ms':>8s} {'error':>7s} {'±video':>7s} {'ppm':>7s}"
    print(header)
    failures = 0
    for case, r in results:
        ok, error = verdict(case, r)
        mark = {True: "", False: "  FAIL", None: ""}[ok]
        failures += ok is False
        if "error" in r:
            print(f"{case.name:32s} {r['error'][:80]}{mark}")
            continue
        s = r["summary"]
        fmt = lambda v, f: "" if v is None else format(v, f)
        print(
            f"{case.name:32s} {r['capture']:6s} {r.get('video_timing') or '':6s} {s['measured']:5d} "
            f"{s['no_video_stamp']:6d} {s['audio_cells']:5d} {fmt(s['mean_ms'], '8.2f'):>8s} {error:>7s} "
            f"{fmt(r.get('video_timing_uncertainty_ms'), '7.2f'):>7s} {fmt(s['skew_ppm'], '7.1f'):>7s}{mark}"
        )
    print(f"\n{len(results) - failures} of {len(results)} cases passed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
