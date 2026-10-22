import json
import os
import subprocess
import sys
from array import array
from pathlib import Path

import pytest

import av_drift_detector as avd


def test_generate_then_analyze_measures_the_injected_offset(tmp_path: Path) -> None:
    output = tmp_path / "sync.mp4"
    progress: list[tuple[int, int]] = []
    summary = avd.generate(
        output,
        fps="29.97",
        duration=3,
        width=640,
        height=360,
        audio_offset=0.025,
        on_progress=lambda done, total: progress.append((done, total)),
    )
    assert summary["frames"] == 90
    assert summary["fps"] == "30000/1001"
    assert summary["output"] == str(output)
    assert len(progress) > 10
    assert progress[-1] == (90, 90)

    report = avd.analyze(output)
    assert report["capture"] == "file"
    assert report["fps"] == "30000/1001"
    assert report["fps_source"] == "barcode"
    assert len(report["frames"]) == 90
    assert report["summary"]["measured"] > 80
    assert report["summary"]["mean_ms"] == pytest.approx(25, abs=0.5)
    measured = [f for f in report["frames"] if f["status"] == "ok"]
    assert all(abs(f["drift_ms"] - 25) < 1 for f in measured)

    window = avd.analyze(str(output), offset=1, duration=1)
    assert 29 <= len(window["frames"]) <= 31
    # The CLI's JSON format.
    assert json.loads(json.dumps(report)) == report


def test_errors() -> None:
    with pytest.raises(avd.AvDriftError, match="(?i)file|directory"):
        avd.analyze("/no/such/file.mp4")
    with pytest.raises(avd.AvDriftError, match="capture mode"):
        avd.analyze("x.mp4", capture="phone")  # type: ignore[arg-type]
    with pytest.raises(avd.AvDriftError, match="frame rate"):
        avd.generate("x.mp4", fps="fast")
    with pytest.raises(avd.AvDriftError, match="needs 230400 bytes"):
        avd.read_frame(b"\0" * 10, 640, 360)
    with pytest.raises(avd.AvDriftError, match="stamp source"):
        avd.analyze_frames([{"time": 0.0, "stamp": {"frame_id": 1, "source": "paper"}}])  # type: ignore[typeddict-item]


def test_without_ffmpeg_file_functions_say_how_to_get_it() -> None:
    script = "import av_drift_detector as a\ntry:\n a.analyze('x.mp4')\nexcept a.AvDriftError as e:\n print(e)"
    env = {**os.environ, "AV_DRIFT_FFPROBE": "/nonexistent/ffprobe"}
    out = subprocess.run([sys.executable, "-c", script], env=env, capture_output=True, text=True, check=True)
    assert "couldn't find `/nonexistent/ffprobe`: install FFmpeg 5.1 or later" in out.stdout


def test_a_failing_progress_callback_is_raised(tmp_path: Path) -> None:
    def stop(done: int, total: int) -> None:
        raise KeyError("stop")

    with pytest.raises(KeyError, match="stop"):
        avd.generate(tmp_path / "x.mp4", duration=0.5, width=320, height=180, on_progress=stop)


def test_frames_and_audio_in_memory() -> None:
    renderer = avd.PatternRenderer(fps="25", width=640, height=360)
    assert (renderer.width, renderer.height) == (640, 360)
    frames: list[avd.CapturedFrame] = []
    for n in range(75):
        luma = renderer.render(n)
        assert len(luma) == 640 * 360
        frames.append({"time": n / 25, "stamp": avd.read_frame(luma, 640, 360)})
    assert frames[42]["stamp"] == {"frame_id": 42, "fps": "25", "tone_base_harmonic": None, "source": "barcode"}

    # The QR code alone, with the barcode strip wiped; any bytes-like plane works.
    luma = bytearray(renderer.render(7))
    luma[: 640 * 100] = bytes([16]) * (640 * 100)
    stamp = avd.read_frame(memoryview(luma), 640, 360)
    assert stamp == {"frame_id": 7, "fps": "25", "tone_base_harmonic": 24, "source": "qr"}
    assert avd.read_frame(luma, 640, 360, try_qr=False) is None

    samples = avd.synthesize_audio(int(48_000 * 3.5), fps="25", delay=0.012)
    assert isinstance(samples, array) and samples.typecode == "f"
    report = avd.analyze_frames(
        frames, {"sample_rate": 48_000, "start_time": 0.0, "samples": samples}, width=640, height=360
    )
    assert report["capture"] == "file"
    assert report["summary"]["measured"] > 60
    assert report["summary"]["mean_ms"] == pytest.approx(12, abs=0.01)


def test_music_and_length_options() -> None:
    with_music = avd.synthesize_audio(4800, fps="25")
    assert with_music != avd.synthesize_audio(4800, fps="25", music=False)
    # The info line shows the length only when it's given.
    plain = avd.PatternRenderer(fps="25", width=640, height=360).render(0)
    assert plain != avd.PatternRenderer(fps="25", width=640, height=360, duration=6).render(0)
    with pytest.raises(avd.AvDriftError, match="duration must be positive"):
        avd.PatternRenderer(duration=0)
