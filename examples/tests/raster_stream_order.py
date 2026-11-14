"""Verify visible raster fragments precede dialog text.

Run after building images_drag:
    python3 examples/tests/raster_stream_order.py target/debug/examples/images_drag

This checks encoded output. Native Rio and iTerm2 visual checks remain separate.
"""

import base64
import re
import struct
import subprocess
import sys


def require(condition, detail):
    if not condition:
        raise AssertionError(detail)


for protocol in ("sixel", "iterm2"):
    for inline in (False, True):
        command = [sys.argv[1], "--" + protocol, "--dump-dialog"]
        if inline:
            command.append("--inline")
        records = subprocess.check_output(command, timeout=30).decode().splitlines()
        require(len(records) == 7, "missing dialog stages")
        for record in records:
            name, encoded = record.split("\t")
            payload = bytes.fromhex(encoded)
            if protocol == "sixel":
                images = list(re.finditer(rb"\x1bP[0-9;]*q(.*?)\x1b\\", payload, re.S))
            else:
                images = list(re.finditer(rb"\x1b\]1337;File=([^:\x07]+):([^\x07]+)\x07", payload))
            footprints = {
                "initial": [(12, 4)],
                "dialog": [(12, 1), (3, 3)],
                "dialog unchanged": [],
                "dialog move": [(12, 2), (6, 2)],
                "dialog origin": [(2, 3), (12, 1)],
                "dialog close": [(12, 4)],
                "remove": [],
            }[name]
            require(len(images) == len(footprints), (protocol, name, "fragment count", len(images)))
            if name == "dialog unchanged":
                require(not payload, (protocol, name, "unexpected output"))
            actual = []
            for image in images:
                if protocol == "sixel":
                    dimensions = re.match(rb'"1;1;(\d+);(\d+)', image.group(1))
                    require(dimensions is not None, (name, "missing raster dimensions"))
                    width, height = map(int, dimensions.groups())
                    require(width % 8 == 0 and height % 16 == 0, (name, "unaligned raster fragment"))
                    actual.append((width // 8, height // 16))
                else:
                    dimensions = re.search(rb"width=(\d+);height=(\d+);", image.group(1))
                    require(dimensions is not None, (name, "missing cell dimensions"))
                    width, height = map(int, dimensions.groups())
                    actual.append((width, height))
                    png = base64.b64decode(image.group(2), validate=True)
                    require(struct.unpack(">II", png[16:24]) == (width * 8, height * 16),
                            (name, "incorrect fragment pixels"))
                if name in ("dialog", "dialog move", "dialog origin"):
                    require(payload.rfind(b"Dialog") >= image.end(), (name, "dialog precedes image"))
            require(actual == footprints, (protocol, name, actual, footprints))
            require(b"\0p" not in payload, (name, "placement metadata escaped"))
        print(f"Verified seven fragment-based dialog stages: protocol={protocol}, inline={inline}.")
