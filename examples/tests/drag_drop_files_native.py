"""Exercise the unified example's file drops and drags through native Kitty.

Build: cargo build -p examples --example drag_drop
Run: kitty +runpy 'import runpy; runpy.run_path("examples/tests/drag_drop_files_native.py", run_name="__main__")'

UNCURSES_DND_FILES_LOCAL=1 uses matching host identities.
The default substitutes a different registration identity to exercise remote
transfers. UNCURSES_DND_PREVIEW=1 tests preview-only drops.
This tests the protocol and PTY, not desktop gestures or an actual SSH host.
"""

import base64
from collections import deque
import fcntl
import os
from pathlib import Path
import pty
import re
import select
import struct
import subprocess
import tempfile
import termios
import time
from urllib.parse import unquote, urlsplit

from kitty import fast_data_types as kitty
from kitty.options.types import defaults


def require(condition, message):
    if not condition:
        raise AssertionError(message)


kitty.set_options(defaults)
local = os.environ.get("UNCURSES_DND_FILES_LOCAL") == "1"
preview = os.environ.get("UNCURSES_DND_PREVIEW") == "1"
replies = deque()
received = bytearray()
response_metadata = None
response_body = bytearray()
frames = re.compile(rb"\x1b]72;([^\x1b]*)(?:\x1b\\)")


def capture(_window, data):
    global response_metadata
    for frame in frames.finditer(data):
        metadata, _, payload = frame[1].partition(b";")
        options = dict(field.split(b"=", 1) for field in metadata.split(b":"))
        replies.append((metadata, payload))
        if response_metadata is None and options.get(b"t") == b"r":
            response_metadata = options
        if response_metadata is None or options.get(b"t", b"r") != b"r":
            continue
        response_body.extend(payload)
        if options.get(b"m", b"0") == b"0":
            first = response_metadata
            if (int(first.get(b"X", b"0")) in (0, 1)
                    and (int(first.get(b"y", b"0")) or int(first.get(b"Y", b"0")))):
                received.extend(base64.b64decode(response_body + b"=" * (-len(response_body) % 4)))
            response_metadata = None
            response_body.clear()


capture(None, b"\x1b]72;t=r:x=2:y=1;YQ\x1b\\"
              b"\x1b]72;t=r:x=2:y=1;Yg\x1b\\"
              b"\x1b]72;t=r:x=2:y=1\x1b\\")
require(received == b"ab", "Independent data messages must decode separately")
received.clear()
replies.clear()


os_window, window = kitty.dnd_test_create_fake_window()
screen = kitty.Screen(None, 24, 100, 100, 8, 16, window)
kitty.dnd_set_test_write_func(capture)


def probe(name):
    return kitty.dnd_test_probe_state(window, name)


with tempfile.TemporaryDirectory(prefix="dnd-native-", dir="target") as directory:
    root = Path(directory).resolve()
    sources = root / "source # tree"
    sources.mkdir()
    (sources / "movie.bin").write_bytes(b"Z" * 10_000)
    (sources / "link").symlink_to("movie.bin")
    (sources / "nested").mkdir()
    (sources / "nested" / "empty").write_bytes(b"")
    images = {}
    for extension in ("png", "jpg"):
        images[extension] = Path(f"examples/examples/support/drag_drop.{extension}").read_bytes()
        (sources / "nested" / f"photo.{extension}").write_bytes(images[extension])
    standalone = root / "standalone.bin"
    standalone.write_bytes(b"standalone")
    broken = root / "broken-link"
    broken.symlink_to("missing-target")
    directory_link = root / "directory-link"
    directory_link.symlink_to(sources.name)
    destination = root / "received"
    command = [
        "target/debug/examples/drag_drop", str(sources), str(standalone),
        str(broken), str(directory_link) + "/",
    ]
    if not preview:
        command.extend(["--receive-dir", str(destination)])
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 100, 800, 384))
    original = termios.tcgetattr(slave)
    os.set_blocking(master, False)
    process = subprocess.Popen(command, stdin=slave, stdout=slave, stderr=slave,
                               env={**os.environ, "TERM": "xterm-kitty"})
    output = bytearray()
    pending_output = bytearray()
    pty_pending = bytearray()
    replied_da = False

    def diagnostics():
        text = re.sub(rb"\x1b\[[0-?]*[ -/]*[@-~]", b"", bytes(output))
        return frames.sub(b"", text).decode("utf-8", "replace")

    def feed_native(pending):
        if not local:
            metadata = pending[5:-2].partition(b";")[0]
            if metadata in (b"t=a:x=1", b"t=o:x=1"):
                pending = b"\x1b]72;" + metadata + b";1:" + b"0" * 64 + b"\x1b\\"
        while pending:
            count = screen.test_commit_write_buffer(pending, screen.test_create_write_buffer())
            require(count > 0, "Kitty parser made no progress")
            pending = pending[count:]
            screen.test_parse_written_data()

    def pump(condition=lambda: True):
        global replied_da
        deadline = time.monotonic() + 10
        while replies or pty_pending or not condition():
            if replies and not pty_pending:
                metadata, payload = replies.popleft()
                pty_pending.extend(payload if metadata is None else
                                   b"\x1b]72;" + metadata + b";" + payload + b"\x1b\\")
            remaining = deadline - time.monotonic()
            require(remaining > 0, f"Timed out:\n{diagnostics()[-3000:]}")
            ready, writable, _ = select.select([master], [master] if pty_pending else [], [], remaining)
            require(ready or writable, f"No PTY progress:\n{diagnostics()[-3000:]}")
            if writable:
                count = os.write(master, pty_pending)
                del pty_pending[:count]
            if not ready:
                continue
            data = os.read(master, 65536)
            require(data, f"PTY closed:\n{diagnostics()[-3000:]}")
            output.extend(data)
            pending_output.extend(data)
            while True:
                start = pending_output.find(b"\x1b]72;")
                if start < 0:
                    del pending_output[:-5]
                    break
                del pending_output[:start]
                end = pending_output.find(b"\x1b\\")
                if end < 0:
                    break
                frame = bytes(pending_output[:end + 2])
                del pending_output[:end + 2]
                feed_native(frame)
            if not replied_da and b"\x1b[c" in output:
                replies.append((None, b"\x1b[?62c"))
                replied_da = True

    try:
        pump(lambda: "Ready for text" in diagnostics())
        incoming = root / "incoming"
        incoming.mkdir()
        content = bytes(range(256)) * 512
        (incoming / "binary").write_bytes(content)
        (incoming / "link").symlink_to("binary")
        (incoming / "nested").mkdir()
        (incoming / "nested" / "empty").write_bytes(b"")
        for extension, data in images.items():
            (incoming / "nested" / f"photo.{extension}").write_bytes(data)
        (incoming / "line\nbreak").write_bytes(b"arbitrary filename")
        kitty.dnd_test_set_mouse_pos(window, 5, 3, 40, 48)
        kitty.dnd_test_fake_drop_event(window, False, ["text/uri-list"], 5, 3, 2)
        pump(lambda: probe("drop_action") == 2)
        kitty.dnd_test_fake_drop_event(window, True, ["text/uri-list"], 5, 3, 2)
        pump(lambda: probe("drop_getting_data_for_mime") == "text/uri-list")
        partial = set(destination.iterdir()) if not preview else set()
        kitty.dnd_test_fake_drop_event(window, False, ["text/uri-list"], 5, 3, 2)
        pump(lambda: probe("drop_action") == 2 and "Drop canceled by a new drag" in diagnostics())
        require(all(not path.exists() for path in partial), "Replacement drop leaked partial files")
        kitty.dnd_test_fake_drop_event(window, True, ["text/uri-list"], 5, 3, 2)
        pump(lambda: probe("drop_getting_data_for_mime") == "text/uri-list")
        kitty.dnd_test_fake_drop_data(window, "text/uri-list", (incoming.as_uri() + "/\r\n").encode())
        pump(lambda: b"\x1b]72;t=r:o=1\x1b\\" in output)
        require(probe("last_drop_action") == 2, diagnostics())
        require(probe("drop_num_dir_handles") == 0, "Directory handles leaked")
        require(probe("drop_file_fd_plus_one") == 0, "File descriptor leaked")
        require(f"URI list: remote={str(not local).lower()}" in diagnostics(), diagnostics())
        expected = len(content) + len(b"binaryarbitrary filename") + sum(map(len, images.values()))
        require(len(received) == expected, f"Received {len(received)}, expected {expected}")
        if preview:
            require(not destination.exists(), "Preview mode created destination files")
        else:
            drops = list(destination.iterdir())
            require(len(drops) == 1, "Each complete drop must have one fresh directory")
            saved = drops[0] / "incoming"
            require((saved / "binary").read_bytes() == content, "Saved file changed")
            require((saved / "line\nbreak").read_bytes() == b"arbitrary filename", "Filename changed")
            require(os.readlink(saved / "link") == "binary", "Saved symlink changed")
            require((saved / "nested" / "empty").read_bytes() == b"", "Saved empty file changed")
            for extension, data in images.items():
                require((saved / "nested" / f"photo.{extension}").read_bytes() == data, "Saved image changed")

        previous_drops = set(destination.iterdir()) if not preview else set()
        output.clear()
        kitty.dnd_test_fake_drop_event(window, True, ["text/uri-list"], 5, 3, 2)
        pump(lambda: probe("drop_getting_data_for_mime") == "text/uri-list")
        kitty.dnd_test_fake_drop_data(
            window, "text/uri-list", ((incoming.as_uri() + "/\r\n") * 10_001).encode()
        )
        pump(lambda: "Drop canceled: drop entry limit exceeded" in diagnostics())
        require(process.poll() is None and probe("drop_wanted"), "Entry limit stopped the example")
        require(probe("last_drop_action") == 0, "Entry limit did not cancel the drop")
        if not preview:
            require(set(destination.iterdir()) == previous_drops, "Entry limit leaked partial storage")

        output.clear()
        kitty.dnd_test_start_drag_offer(window, 5, 3, 40, 48)
        pump(lambda: "Drag started" in diagnostics())
        require(probe("drag_thumbnail_size") == 16 * 16 * 4, diagnostics())
        kitty.dnd_test_force_drag_dropped(window)
        if local:
            try:
                kitty.dnd_test_drag_get_data(window, "text/uri-list")
                raise AssertionError("Expected fresh URI data")
            except BlockingIOError:
                pass
        else:
            kitty.dnd_test_request_drag_data(window, 0)
        pump(lambda: b"\x1b]72;t=e:y=0\x1b\\" in output if local
             else probe("drag_remote_data_complete") is None)
        uris = kitty.dnd_test_drag_get_data(window, "text/uri-list")
        paths = [Path(unquote(urlsplit(uri).path)) for uri in uris.decode().splitlines() if uri.startswith("file:")]
        require(len(paths) == 4, repr(uris))
        tree, file, link, linked_tree = paths
        require(tree.name == sources.name, "Source name changed")
        require((tree / "movie.bin").read_bytes() == b"Z" * 10_000, "Outgoing bytes changed")
        require(os.readlink(tree / "link") == "movie.bin", "Outgoing symlink changed")
        require(file.read_bytes() == b"standalone", "Second root changed")
        require(os.readlink(link) == "missing-target", "Broken root link changed")
        require((linked_tree / "movie.bin").read_bytes() == b"Z" * 10_000,
                "Trailing-separator link did not transfer its directory")
        if local:
            require(uris.decode().splitlines()[3].endswith("/"), "Directory URI lost its trailing separator")
        else:
            require(not linked_tree.is_symlink(), "Remote directory request sent a symlink")
        for extension, data in images.items():
            require((tree / "nested" / f"photo.{extension}").read_bytes() == data, "Outgoing nested image changed")
        kitty.dnd_test_drag_finish(window, False)
        pump(lambda: "finished: canceled=" in diagnostics())
        if not local:
            for index in range(150):
                directory = sources / f"wide-{index:03}"
                directory.mkdir()
                (directory / "leaf").write_bytes(b"leaf")
            output.clear()
            kitty.dnd_test_start_drag_offer(window, 5, 3, 40, 48)
            pump(lambda: b"\x1b]72;t=P:x=-1\x1b\\" in output)
            kitty.dnd_test_force_drag_dropped(window)
            kitty.dnd_test_request_drag_data(window, 0)
            pump(lambda: "Drag canceled: outgoing directory handle limit exceeded" in diagnostics())
            require(process.poll() is None and probe("drag_can_offer"), "Handle limit stopped the example")
        print("Verified default entry limits, partial cleanup, and available registrations.")
        if not local:
            print("Verified default handle limits during a wide outgoing tree.")
        pty_pending.extend(b"q")
        pump(lambda: b"\x1b[?1049l" in output)
        require(process.wait(timeout=5) == 0, diagnostics())
        restored = termios.tcgetattr(slave)
        original[3] &= ~termios.PENDIN
        restored[3] &= ~termios.PENDIN
        require(original == restored, "Terminal attributes changed")
        print(f"Native bidirectional files passed: local={local}, preview={preview}, nested images, symlinks, replacement cleanup.")
    finally:
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=5)
        os.close(master)
        os.close(slave)
        kitty.dnd_set_test_write_func(None)
        kitty.dnd_test_cleanup_fake_window(os_window)
