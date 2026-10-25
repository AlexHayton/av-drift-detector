# The Rust extension module (bindings/python/src/lib.rs). Use the wrappers in
# av_drift_detector instead: these return reports as JSON strings.

from collections.abc import Buffer, Callable, Iterable
from typing import Any

class AvDriftError(Exception):
    """An analysis, generation or option error from av-drift-detector."""

class PatternRenderer:
    """Renders test-pattern frames as 8-bit luma planes, without FFmpeg."""

    def __init__(
        self,
        *,
        fps: str = "30",
        width: int = 1280,
        height: int = 720,
        sample_rate: int = 48000,
        duration: float | None = None,
    ) -> None: ...
    @property
    def width(self) -> int: ...
    @property
    def height(self) -> int: ...
    def render(self, frame_id: int) -> bytes:
        """Frame ``frame_id`` as ``width × height`` bytes of luma (16 = black, 235 = white)."""

class StampReader:
    """Reads the stamps of consecutive frames of one capture."""

    def __init__(self, *, try_qr: bool = True) -> None: ...
    def read(
        self, luma: Buffer, width: int, height: int, time: float, stride: int | None = None
    ) -> Any: ...
    def finish(self) -> list[Any]: ...

def analyze(
    path: str,
    *,
    offset: float = 0.0,
    duration: float | None = None,
    fps: str | None = None,
    use_qr: bool = True,
    capture: str = "auto",
) -> str: ...
def generate(
    output: str,
    *,
    fps: str = "30",
    duration: float = 10.0,
    width: int = 1280,
    height: int = 720,
    sample_rate: int = 48000,
    video_codec: str = "h264",
    audio_codec: str = "aac",
    audio_offset: float = 0.0,
    music: bool = True,
    on_progress: Callable[[int, int], object] | None = None,
) -> dict[str, Any]: ...
def read_frame(
    luma: Buffer, width: int, height: int, stride: int | None = None, try_qr: bool = True
) -> dict[str, Any] | None: ...
def analyze_frames(
    frames: Iterable[Any],
    audio: Any = None,
    *,
    width: int = 0,
    height: int = 0,
    offset: float = 0.0,
    duration: float | None = None,
    fps: str | None = None,
    capture: str = "auto",
) -> str: ...
def synthesize_audio(
    count: int,
    *,
    fps: str = "30",
    sample_rate: int = 48000,
    delay: float = 0.0,
    start: int = 0,
    music: bool = True,
) -> bytes: ...
