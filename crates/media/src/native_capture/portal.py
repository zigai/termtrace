"""Process-owned portal/PipeWire capture; stdout is reserved for diagnostics.

Run with the system Python providing PyGObject, Gio, Gst and GstVideo. This
helper is embedded in the Rust binary; it is not part of the public Python SDK.
The GI objects are the dynamic boundary to the desktop's installed libraries.
"""

from __future__ import annotations

import json
import os
import select
import signal
import struct
import sys
import time
import uuid
import zlib
from pathlib import Path
from types import ModuleType
from typing import Any

import gi


def native_bindings() -> tuple[ModuleType, ModuleType, ModuleType, ModuleType]:
    """Select ABI versions before loading the introspected native libraries."""
    gi.require_version("Gio", "2.0")
    gi.require_version("Gst", "1.0")
    gi.require_version("GstVideo", "1.0")
    from gi.repository import Gio, GLib, Gst, GstVideo

    return Gio, GLib, Gst, GstVideo


Gio, GLib, Gst, GstVideo = native_bindings()


class Portal:
    """Own the D-Bus connection, portal session and restricted PipeWire FD."""

    def __init__(self) -> None:
        self.bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
        self.session: str | None = None
        self.fd: int | None = None
        self.closed = False
        self.subscription: int | None = None

    # GI's variant API is dynamically typed; keep that escape hatch at this boundary.
    def request(
        self, method: str, signature: str, args: tuple[Any, ...]
    ) -> dict[str, Any]:
        token = "termtrace_" + uuid.uuid4().hex
        options = args[-1] | {"handle_token": GLib.Variant("s", token)}
        sender = self.bus.get_unique_name()[1:].replace(".", "_")
        path = f"/org/freedesktop/portal/desktop/request/{sender}/{token}"
        response: list[tuple[int, dict[str, Any]]] = []

        def received(
            connection: Gio.DBusConnection,
            sender_name: str,
            object_path: str,
            interface: str,
            name: str,
            parameters: GLib.Variant,
            data: object,
        ) -> None:
            response.append(parameters.unpack())

        subscription = self.bus.signal_subscribe(
            "org.freedesktop.portal.Desktop",
            "org.freedesktop.portal.Request",
            "Response",
            path,
            None,
            Gio.DBusSignalFlags.NONE,
            received,
            None,
        )
        try:
            handle = self.bus.call_sync(
                "org.freedesktop.portal.Desktop",
                "/org/freedesktop/portal/desktop",
                "org.freedesktop.portal.ScreenCast",
                method,
                GLib.Variant(signature, (*args[:-1], options)),
                GLib.VariantType.new("(o)"),
                Gio.DBusCallFlags.NONE,
                10000,
                None,
            ).unpack()[0]
            if handle != path:
                raise RuntimeError("portal did not honor the request handle token")
            deadline = time.monotonic() + 120
            context = GLib.MainContext.default()
            while not response:
                while context.pending():
                    context.iteration(False)
                if time.monotonic() >= deadline or self.closed:
                    raise RuntimeError("portal selection timed out or session closed")
                if select.select([sys.stdin], [], [], 0.02)[0]:
                    raise RuntimeError(
                        "capture cancelled before portal selection completed"
                    )
            code, result = response[0]
            if code != 0:
                raise RuntimeError(
                    f"portal request {method} failed with response {code}"
                )
            return result
        finally:
            self.bus.signal_unsubscribe(subscription)
            if not response:
                self.bus.call_sync(
                    "org.freedesktop.portal.Desktop",
                    path,
                    "org.freedesktop.portal.Request",
                    "Close",
                    None,
                    None,
                    Gio.DBusCallFlags.NONE,
                    1000,
                    None,
                )

    def open(self) -> tuple[int, dict[str, Any]]:
        result = self.request(
            "CreateSession",
            "(a{sv})",
            (
                {
                    "session_handle_token": GLib.Variant(
                        "s", "termtrace_" + uuid.uuid4().hex
                    ),
                },
            ),
        )
        session = result.get("session_handle")
        if not isinstance(session, str) or not session.startswith(
            "/org/freedesktop/portal/"
        ):
            raise RuntimeError("portal returned an invalid session handle")
        self.session = session

        def closed(
            connection: Gio.DBusConnection,
            sender_name: str,
            object_path: str,
            interface: str,
            name: str,
            parameters: GLib.Variant,
            data: object,
        ) -> None:
            self.closed = True

        self.subscription = self.bus.signal_subscribe(
            "org.freedesktop.portal.Desktop",
            "org.freedesktop.portal.Session",
            "Closed",
            session,
            None,
            Gio.DBusSignalFlags.NONE,
            closed,
            None,
        )
        self.request(
            "SelectSources",
            "(oa{sv})",
            (
                session,
                {
                    "types": GLib.Variant("u", 2),
                    "multiple": GLib.Variant("b", False),
                },
            ),
        )
        result = self.request("Start", "(osa{sv})", (session, "", {}))
        streams = result.get("streams")
        if not isinstance(streams, list) or len(streams) != 1:
            raise RuntimeError("portal must provide exactly one selected window")
        node, properties = streams[0]
        if not isinstance(node, int) or not isinstance(properties, dict):
            raise TypeError("portal returned an invalid PipeWire stream")
        result, descriptors = self.bus.call_with_unix_fd_list_sync(
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.ScreenCast",
            "OpenPipeWireRemote",
            GLib.Variant("(oa{sv})", (session, {})),
            GLib.VariantType.new("(h)"),
            Gio.DBusCallFlags.NONE,
            10000,
            None,
            None,
        )
        self.fd = descriptors.get(result.unpack()[0])
        return node, properties

    def close(self) -> None:
        try:
            if self.session is not None and not self.closed:
                try:
                    self.bus.call_sync(
                        "org.freedesktop.portal.Desktop",
                        self.session,
                        "org.freedesktop.portal.Session",
                        "Close",
                        None,
                        None,
                        Gio.DBusCallFlags.NONE,
                        1000,
                        None,
                    )
                except GLib.Error as e:
                    # Closing an outstanding request can already destroy its session.
                    if Gio.DBusError.get_remote_error(e) not in {
                        "org.freedesktop.DBus.Error.UnknownMethod",
                        "org.freedesktop.DBus.Error.UnknownObject",
                    }:
                        raise
        finally:
            self.closed = True
            if self.subscription is not None:
                self.bus.signal_unsubscribe(self.subscription)
            if self.fd is not None:
                os.close(self.fd)
                self.fd = None


class Acquisition:
    """Bound memory to one RGB frame; stream timestamps before encoding each frame."""

    def __init__(self, directory: Path, width: int, height: int) -> None:
        self.directory = directory
        self.width = width
        self.height = height
        self.count = 0
        self.first_received: int | None = None
        self.last_pts: int | None = None
        self.last_pixels: bytes | None = None
        self.last_stride = 0
        self.failure: Exception | None = None
        self.log = (directory / "acquisition-map.jsonl").open("x", encoding="utf-8")
        self.pipeline = Gst.parse_launch(
            "pipewiresrc name=source ! videoconvert ! videoscale ! "
            f"video/x-raw,format=RGB,width={width},height={height} ! "
            "identity name=frames ! videoconvert ! video/x-raw,format=Y444 ! "
            "x264enc pass=quant quantizer=0 speed-preset=ultrafast bframes=0 "
            "threads=1 ! matroskamux ! filesink name=output"
        )
        self.pipeline.get_by_name("output").set_property(
            "location", str(directory / "session.mkv")
        )
        self.pipeline.get_by_name("frames").get_static_pad("src").add_probe(
            Gst.PadProbeType.BUFFER,
            self.frame,
        )

    def frame(self, pad: Gst.Pad, info: Gst.PadProbeInfo) -> Gst.PadProbeReturn:
        try:
            buffer = info.get_buffer()
            received = time.time_ns()
            monotonic = time.monotonic_ns()
            if buffer.pts == Gst.CLOCK_TIME_NONE or (
                self.last_pts is not None and buffer.pts < self.last_pts
            ):
                raise RuntimeError("PipeWire frame has missing or decreasing PTS")
            video = GstVideo.VideoInfo.new_from_caps(pad.get_current_caps())
            if video.width != self.width or video.height != self.height:
                raise RuntimeError("capture frame geometry changed")
            pixels = buffer.extract_dup(0, buffer.get_size())
            metadata = GstVideo.buffer_get_video_meta(buffer)
            stride = metadata.stride[0] if metadata is not None else video.stride[0]
            offset = metadata.offset[0] if metadata is not None else video.offset[0]
            if stride < self.width * 3 or len(pixels) < offset + stride * self.height:
                raise RuntimeError("capture frame is incomplete")
            self.log.write(
                json.dumps(
                    {
                        "frame": self.count,
                        "source_pts_ns": buffer.pts,
                        "source_duration_ns": None
                        if buffer.duration == Gst.CLOCK_TIME_NONE
                        else buffer.duration,
                        "received_unix_ns": received,
                        "received_monotonic_ns": monotonic,
                        "correlation": "pipewire_pts_and_collector_receipt_no_presentation_claim",
                    }
                )
                + "\n"
            )
            self.log.flush()
            self.last_pts = buffer.pts
            self.last_pixels = pixels[offset:]
            self.last_stride = stride
            self.count += 1
            if self.first_received is None:
                self.first_received = received
                temporary = self.directory / ".acquisition-ready.tmp"
                temporary.write_text(
                    json.dumps({"first_received_unix_ns": received}) + "\n"
                )
                temporary.rename(self.directory / "acquisition-ready.json")
            return Gst.PadProbeReturn.OK
        except Exception as e:  # noqa: BLE001 - callback boundary; finish() re-raises it
            # PyGObject does not propagate callback exceptions to the pipeline owner.
            self.failure = e
            return Gst.PadProbeReturn.DROP

    def finish(self) -> None:
        if self.failure is not None:
            raise self.failure
        if self.last_pixels is None:
            raise RuntimeError("capture contains no acquired frames")

        def chunk(kind: bytes, data: bytes) -> bytes:
            return (
                struct.pack(">I", len(data))
                + kind
                + data
                + struct.pack(">I", zlib.crc32(kind + data))
            )

        compressor = zlib.compressobj()
        with (self.directory / "final.png").open("xb") as image:
            image.write(b"\x89PNG\r\n\x1a\n")
            image.write(
                chunk(
                    b"IHDR",
                    struct.pack(">IIBBBBB", self.width, self.height, 8, 2, 0, 0, 0),
                )
            )
            for row in range(self.height):
                start = row * self.last_stride
                compressed = compressor.compress(
                    b"\0" + self.last_pixels[start : start + self.width * 3]
                )
                if compressed:
                    image.write(chunk(b"IDAT", compressed))
            image.write(chunk(b"IDAT", compressor.flush()))
            image.write(chunk(b"IEND", b""))
        (self.directory / "acquisition-completion.json").write_text(
            json.dumps(
                {
                    "frames": self.count,
                    "first_received_unix_ns": self.first_received,
                    "last_source_pts_ns": self.last_pts,
                    "gstreamer_version": Gst.version_string(),
                    "screenshot_frame": self.count - 1,
                }
            )
            + "\n"
        )

    def run(self, portal: Portal) -> None:
        bus = self.pipeline.get_bus()
        self.pipeline.set_state(Gst.State.PLAYING)
        stopping: float | None = None
        context = GLib.MainContext.default()
        while True:
            while context.pending():
                context.iteration(False)
            if self.failure is not None:
                raise self.failure
            if portal.closed:
                raise RuntimeError("portal session closed during capture")
            if stopping is None and select.select([sys.stdin], [], [], 0)[0]:
                if os.read(sys.stdin.fileno(), 1) != b"q":
                    raise RuntimeError("capture owner disconnected")
                self.pipeline.send_event(Gst.Event.new_eos())
                stopping = time.monotonic()
            if stopping is not None and time.monotonic() - stopping > 10:
                raise RuntimeError("capture encoder did not finish within 10 seconds")
            message = bus.timed_pop_filtered(
                20 * Gst.MSECOND, Gst.MessageType.ERROR | Gst.MessageType.EOS
            )
            if message is not None:
                if message.type == Gst.MessageType.ERROR:
                    error, _ = message.parse_error()
                    raise RuntimeError(f"capture pipeline failed: {error.message}")
                if stopping is None:
                    raise RuntimeError(
                        "capture stream ended before the owner requested stop"
                    )
                break
        self.pipeline.set_state(Gst.State.NULL)
        self.finish()

    def close(self) -> None:
        self.pipeline.set_state(Gst.State.NULL)
        self.log.close()


def main() -> None:
    os.umask(0o077)
    Gst.init(None)
    directory = Path(sys.argv[1])
    width, height = int(sys.argv[2]), int(sys.argv[3])
    if not directory.is_dir() or not 1 <= width <= 8192 or not 1 <= height <= 8192:
        raise ValueError("invalid private capture directory or dimensions")

    def terminate(number: int, frame: object) -> None:
        raise SystemExit(1)

    signal.signal(signal.SIGTERM, terminate)
    portal = Portal()
    acquisition: Acquisition | None = None
    try:
        node, properties = portal.open()
        (directory / "portal-source.json").write_text(
            json.dumps(
                {
                    "node_id": node,
                    "properties": properties,
                    "selection": "user_selected_window_not_independently_verified",
                }
            )
            + "\n"
        )
        acquisition = Acquisition(directory, width, height)
        source = acquisition.pipeline.get_by_name("source")
        source.set_property("fd", portal.fd)
        if source.find_property("on-disconnect") is not None:
            source.set_property("on-disconnect", 2)
        serial = properties.get("pipewire-serial")
        if serial is not None:
            source.set_property("target-object", str(serial))
        else:
            source.set_property("path", str(node))
        acquisition.run(portal)
    finally:
        if acquisition is not None:
            acquisition.close()
        portal.close()


if __name__ == "__main__":
    main()
