"""Exercise the real GStreamer encoder without opening the desktop chooser."""

from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from types import ModuleType


def load_capture() -> ModuleType:
    path = (
        Path(__file__).resolve().parents[1]
        / "crates/media/src/native_capture/portal.py"
    )
    spec = importlib.util.spec_from_file_location("termtrace_portal", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


capture = load_capture()
capture.Gst.init(None)


class AcquisitionTests(unittest.TestCase):
    def test_encoded_frames_match_acquired_pts_and_final_pixels(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            # An odd width exercises RGB row padding, which must not enter PNG rows.
            acquisition = capture.Acquisition(root, 65, 32)
            try:
                source = acquisition.pipeline.get_by_name("source")
                peer = source.get_static_pad("src").get_peer().get_parent_element()
                source.unlink(peer)
                acquisition.pipeline.remove(source)
                test_source = capture.Gst.ElementFactory.make("videotestsrc", "fixture")
                test_source.set_property("num-buffers", 6)
                test_source.set_property("pattern", "solid-color")
                test_source.set_property("foreground-color", 0xFFFF0000)
                acquisition.pipeline.add(test_source)
                self.assertTrue(test_source.link(peer))
                acquisition.pipeline.set_state(capture.Gst.State.PLAYING)
                message = acquisition.pipeline.get_bus().timed_pop_filtered(
                    10 * capture.Gst.SECOND,
                    capture.Gst.MessageType.ERROR | capture.Gst.MessageType.EOS,
                )
                self.assertIsNotNone(message)
                if message.type == capture.Gst.MessageType.ERROR:
                    self.fail(message.parse_error())
                acquisition.pipeline.set_state(capture.Gst.State.NULL)
                self.assertIsNone(acquisition.failure)
                acquisition.finish()
            finally:
                acquisition.close()
            frames = [
                json.loads(line)
                for line in (root / "acquisition-map.jsonl").read_text().splitlines()
            ]
            self.assertEqual([frame["frame"] for frame in frames], list(range(6)))
            self.assertEqual(frames[0]["source_pts_ns"], 0)
            self.assertEqual(frames[-1]["source_pts_ns"], 166666666)
            probe = subprocess.run(
                [
                    "ffprobe",
                    "-v",
                    "error",
                    "-count_packets",
                    "-show_streams",
                    "-of",
                    "json",
                    str(root / "session.mkv"),
                ],
                check=True,
                capture_output=True,
                timeout=10,
            )
            self.assertEqual(
                json.loads(probe.stdout)["streams"][0]["nb_read_packets"], "6"
            )
            pixels = subprocess.run(
                [
                    "ffmpeg",
                    "-v",
                    "error",
                    "-i",
                    str(root / "final.png"),
                    "-f",
                    "rawvideo",
                    "-pix_fmt",
                    "rgb24",
                    "-",
                ],
                check=True,
                capture_output=True,
                timeout=10,
            ).stdout
            self.assertEqual(pixels, b"\xff\0\0" * (65 * 32))
            completed = json.loads((root / "acquisition-completion.json").read_text())
            ready = json.loads((root / "acquisition-ready.json").read_text())
            self.assertEqual(completed["frames"], 6)
            self.assertEqual(completed["screenshot_frame"], 5)
            self.assertEqual(
                completed["first_received_unix_ns"], ready["first_received_unix_ns"]
            )


if __name__ == "__main__":
    unittest.main()
