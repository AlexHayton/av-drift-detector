"""Measure audio/video sync to a fraction of a millisecond, frame by frame.

Python bindings to the Rust library behind the ``av-drift`` command. Generate
a test video, play it through whatever you're testing, record the result, and
measure how far the sound has drifted from the picture at every frame::

    import av_drift_detector as avd

    avd.generate("sync.mp4", fps="29.97", duration=10, audio_offset=0.040)
    report = avd.analyze("sync.mp4")
    print(report["summary"]["mean_ms"])  # 40.0: positive means the audio is late

Reports are the same dicts ``av-drift analyze --json`` writes, so
``json.dump(report, f)`` gives the CLI's format. Reading and writing files
runs the ``ffmpeg`` and ``ffprobe`` programs (FFmpeg 5.1 or later, on ``PATH``
or at ``AV_DRIFT_FFMPEG`` and ``AV_DRIFT_FFPROBE``); the in-memory functions
(``read_frame``, ``analyze_frames``, ``PatternRenderer``,
``synthesize_audio``) don't need them.
"""

from __future__ import annotations

import array
import json
import os
from collections.abc import Callable, Iterable
from importlib.metadata import PackageNotFoundError, version
from typing import TYPE_CHECKING, Literal, NotRequired, TypedDict

if TYPE_CHECKING:
    from collections.abc import Buffer  # Python 3.12+; annotations only

from . import _native
from ._native import AvDriftError, PatternRenderer

__all__ = [
    "AvDriftError",
    "CapturedAudio",
    "CapturedFrame",
    "DriftReport",
    "DriftSummary",
    "FrameDrift",
    "FrameStamp",
    "GenerateSummary",
    "PatternRenderer",
    "analyze",
    "analyze_frames",
    "generate",
    "read_frame",
    "synthesize_audio",
]

try:
    __version__ = version("av-drift-detector")
except PackageNotFoundError:  # pragma: no cover - running from a source tree
    __version__ = "0+unknown"

Capture = Literal["auto", "file", "camera"]


class FrameStamp(TypedDict):
    """A frame number read from a picture."""

    frame_id: int
    #: The pattern's exact frame rate (``"30000/1001"``), if the stamp carries it.
    fps: str | None
    #: The audio tone plan's base harmonic (QR codes only).
    tone_base_harmonic: int | None
    source: Literal["barcode", "qr"]


class FrameDrift(TypedDict):
    """Drift measured at one captured video frame."""

    #: Presentation time of the captured frame, in seconds.
    time: float
    video_frame: int | None
    video_position: float | None
    stamp_source: Literal["barcode", "qr"] | None
    audio_frame: float | None
    #: Video minus audio in ms: positive means the audio is late.
    drift_ms: float | None
    dropped_before: int
    repeated: bool
    status: Literal["ok", "no_video_stamp", "no_audio"]


class DriftSummary(TypedDict):
    """Statistics over a report's frames (drift in ms; positive = audio late)."""

    frames: int
    measured: int
    mean_ms: float | None
    median_ms: float | None
    min_ms: float | None
    max_ms: float | None
    stddev_ms: float | None
    skew_ms_per_min: float | None
    skew_ppm: float | None
    dropped_frames: int
    repeated_frames: int
    backward_jumps: int
    no_video_stamp: int
    no_audio: int
    audio_cells: int
    audio_outliers: int


class DriftReport(TypedDict):
    """The result of analysing a capture, as ``av-drift analyze --json`` writes it."""

    input: str
    #: The pattern's exact frame rate, such as ``"30000/1001"``.
    fps: str
    fps_source: Literal["barcode", "qr", "option"]
    capture: Literal["file", "camera"]
    #: Camera captures: ± how well the display's frame timing is known.
    video_timing_uncertainty_ms: float | None
    width: int
    height: int
    offset: float
    duration: float | None
    summary: DriftSummary
    frames: list[FrameDrift]


class GenerateSummary(TypedDict):
    """What :func:`generate` wrote."""

    output: str
    frames: int
    fps: str
    duration: float
    width: int
    height: int
    audio_offset: float


class CapturedFrame(TypedDict):
    """A frame you decoded: when it was shown (seconds) and its stamp, if readable."""

    time: float
    stamp: NotRequired[FrameStamp | None]


class CapturedAudio(TypedDict):
    """Mono audio you decoded. ``samples`` is float32: an ``array("f")``, a numpy array, ..."""

    sample_rate: int
    start_time: float
    samples: Buffer


def analyze(
    path: str | os.PathLike[str],
    *,
    offset: float = 0.0,
    duration: float | None = None,
    fps: str | None = None,
    use_qr: bool = True,
    capture: Capture = "auto",
) -> DriftReport:
    """Measures the A/V drift of every frame in a capture of a test video.

    Measures as ``av-drift analyze`` does, decoding with the ``ffmpeg``
    program. ``offset`` and
    ``duration`` (seconds) choose a window; ``fps`` (``"29.97"``,
    ``"30000/1001"``) overrides the rate the stamps carry; ``capture`` says
    how frames are timed (``"auto"`` picks camera mode when most frames were
    read from the QR code).
    """
    report = _native.analyze(
        os.fspath(path), offset=offset, duration=duration, fps=fps, use_qr=use_qr, capture=capture
    )
    return json.loads(report)


def generate(
    output: str | os.PathLike[str],
    *,
    fps: str = "30",
    duration: float = 10.0,
    width: int = 1280,
    height: int = 720,
    sample_rate: int = 48_000,
    video_codec: Literal["h264", "ffv1"] = "h264",
    audio_codec: Literal["aac", "pcm"] = "aac",
    audio_offset: float = 0.0,
    on_progress: Callable[[int, int], object] | None = None,
) -> GenerateSummary:
    """Writes a test video (``.mp4``, ``.mov`` or ``.mkv``).

    Every frame shows its number as a barcode, a QR code and timecode, and
    every frame's worth of audio carries it as tones. ``audio_offset``
    (seconds) deliberately delays the audio; negative makes it early.
    ``on_progress(frames_done, frames_total)`` is called about once a percent.
    FFV1 video and PCM audio need ``.mov`` or ``.mkv``. Encodes with the
    ``ffmpeg`` program; H.264 needs a build with libx264.
    """
    return _native.generate(
        os.fspath(output),
        fps=fps,
        duration=duration,
        width=width,
        height=height,
        sample_rate=sample_rate,
        video_codec=video_codec,
        audio_codec=audio_codec,
        audio_offset=audio_offset,
        on_progress=on_progress,
    )


def read_frame(
    luma: Buffer, width: int, height: int, stride: int | None = None, *, try_qr: bool = True
) -> FrameStamp | None:
    """Reads the frame stamp from an 8-bit luma (Y) plane: the barcode, then
    (unless ``try_qr`` is false) the QR code. Returns None if neither reads.

    ``luma`` is any bytes-like object: ``bytes``, a ``memoryview``, a numpy
    ``uint8`` array, ...
    """
    return _native.read_frame(luma, width, height, stride, try_qr)


def analyze_frames(
    frames: Iterable[CapturedFrame],
    audio: CapturedAudio | None = None,
    *,
    width: int = 0,
    height: int = 0,
    offset: float = 0.0,
    duration: float | None = None,
    fps: str | None = None,
    capture: Capture = "auto",
) -> DriftReport:
    """Measures drift from frames and audio you decoded yourself (a capture
    card, WebRTC, a camera). Stamps come from :func:`read_frame`; frame times
    and ``audio["start_time"]`` must be on the same clock, in seconds.
    """
    report = _native.analyze_frames(
        frames,
        audio,
        width=width,
        height=height,
        offset=offset,
        duration=duration,
        fps=fps,
        capture=capture,
    )
    return json.loads(report)


def synthesize_audio(
    count: int, *, fps: str = "30", sample_rate: int = 48_000, delay: float = 0.0, start: int = 0
) -> array.array[float]:
    """The test pattern's audio: ``count`` mono float32 samples from sample
    ``start``, delayed by ``delay`` seconds (positive = late).
    """
    samples = array.array("f")
    samples.frombytes(
        _native.synthesize_audio(count, fps=fps, sample_rate=sample_rate, delay=delay, start=start)
    )
    return samples
