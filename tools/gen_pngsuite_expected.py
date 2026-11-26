#!/usr/bin/env python3
"""Creates tests/pngsuite/expected.bin and expected.txt.

The expected pixels come from ImageMagick (an independent PNG decoder).
Pixels are requested as 16-bit RGBA and reduced to 8 bits with the same
rounding rule as the Rust decoder, so 16-bit files can be compared exactly.

Usage: python3 tools/gen_pngsuite_expected.py
Requires: ImageMagick ("magick" in PATH).
"""
import pathlib
import subprocess
import struct

root = pathlib.Path(__file__).resolve().parent.parent / "tests" / "pngsuite"
index = []
blob = bytearray()
for path in sorted(root.glob("*.png")):
    if path.name.startswith("x"):
        continue  # "x*" files are broken on purpose, there is nothing to compare.
    size = subprocess.run(
        ["magick", "identify", "-format", "%w %h", str(path) + "[0]"],
        capture_output=True, text=True, check=True).stdout.split()
    w, h = int(size[0]), int(size[1])
    raw = subprocess.run(
        ["magick", str(path) + "[0]", "-depth", "16", "-endian", "MSB", "rgba:-"],
        capture_output=True, check=True).stdout
    assert len(raw) == w * h * 8, (path.name, len(raw), w, h)
    out = bytearray()
    for i in range(0, len(raw), 2):
        v = struct.unpack(">H", raw[i:i + 2])[0]
        out.append((v * 255 + 32767) // 65535)
    index.append(f"{path.name} {w} {h} {len(blob)}")
    blob += out
(root / "expected.bin").write_bytes(bytes(blob))
(root / "expected.txt").write_text("\n".join(index) + "\n")
print(len(index), "files,", len(blob), "bytes")
