"""Prove the PyO3 bindings: composite() and stroke() from Python.

    cd overcast-spike/overcast-engine/py
    python -m venv .venv && . .venv/bin/activate && pip install maturin
    maturin develop --release
    python demo.py [out_dir]
"""

import json
import os
import struct
import sys
import time

import overcast_engine as oe

PNG_MAGIC = b"\x89PNG\r\n\x1a\n"


def png_info(data: bytes):
    """(width, height, bit_depth) from a PNG's IHDR."""
    assert data[:8] == PNG_MAGIC, "not a PNG"
    w, h, depth = struct.unpack(">IIB", data[16:25])
    return w, h, depth


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else "out"
    os.makedirs(out, exist_ok=True)

    stack = {
        "depth": "u16",
        "layers": [
            {"type": "fill", "name": "Fill", "color": [0.85, 0.55, 0.35, 1.0]},
            {
                "type": "paint",
                "name": "Paint",
                "strokes": [
                    {"size": 0.09, "hardness": 0.0, "flow": 0.6, "color": [0.1, 0.25, 0.75, 1],
                     "points": [[0.1, 0.3, 1], [0.5, 0.45, 1], [0.9, 0.3, 1]]},
                    {"size": 0.06, "tip": {"shape": "square"}, "angle": 30, "spacing": 1.4,
                     "color": [0.15, 0.6, 0.2, 1], "points": [[0.1, 0.62, 1], [0.9, 0.87, 1]]},
                ],
            },
            {
                "type": "hue_saturation", "name": "Hue/Sat", "hue": 150, "saturation": 40,
                "mask": [{"size": 0.3, "hardness": 0.2, "color": [1, 1, 1, 1],
                          "points": [[0.7, 0.1, 1], [0.7, 0.9, 1]]}],
            },
        ],
    }
    stack_json = json.dumps(stack)

    # composite(): one call, JSON in, PNG out (GIL released while it runs).
    for size, bits in [(512, 8), (2048, 8), (2048, 16)]:
        t = time.perf_counter()
        png = oe.composite(stack_json, size, bit_depth=bits)
        ms = (time.perf_counter() - t) * 1e3
        assert png_info(png) == (size, size, bits), png_info(png)
        name = os.path.join(out, f"py_composite_{size}_{bits}bit.png")
        with open(name, "wb") as f:
            f.write(png)
        print(f"composite {size}² {bits}-bit: {ms:.0f} ms, {len(png)} bytes -> {name}")

    # stroke(): one stroke on a transparent surface, with its damage rect and tile count.
    stroke = {"size": 0.05, "tip": {"shape": "square"}, "angle": 45, "points": [[0.1, 0.1, 1], [0.3, 0.2, 1]]}
    png, rect, tiles = oe.stroke(json.dumps(stroke), 2048)
    assert png_info(png) == (2048, 2048, 8)
    assert rect[0] < rect[2] and rect[1] < rect[3], rect
    assert 1 <= tiles < 64, tiles  # sparse: a 2048² surface has 64 tiles
    print(f"stroke: damage {rect}, {tiles}/64 tiles allocated")

    # Live Stack: incremental strokes into a paint layer and a mask, then composite.
    s = oe.Stack(stack_json, 1024)
    before = s.composite()
    rect, tiles = s.stroke(1, json.dumps({"size": 0.04, "color": [1, 1, 1, 1], "points": [[0.2, 0.8, 1], [0.8, 0.8, 1]]}))
    print(f"Stack.stroke on layer 1: damage {rect}, paint layer now has {tiles} tiles")
    s.stroke(2, json.dumps({"size": 0.2, "color": [1, 1, 1, 1], "points": [[0.2, 0.2, 1]]}), target="mask")
    after = s.composite()
    assert before != after
    assert len(s) == 3

    # Errors surface as ValueError, never a crash.
    for bad in [lambda: oe.composite("{", 64), lambda: oe.composite(stack_json, 0),
                lambda: s.stroke(0, json.dumps({"size": 0.1, "points": [[0.5, 0.5, 1]]})),
                lambda: s.stroke(9, json.dumps({"size": 0.1, "points": [[0.5, 0.5, 1]]})),
                lambda: oe.composite(stack_json, 64, bit_depth=12)]:
        try:
            bad()
        except ValueError as e:
            print(f"ValueError as expected: {e}")
        else:
            raise AssertionError("expected ValueError")
    print("python demo: OK")


if __name__ == "__main__":
    main()
