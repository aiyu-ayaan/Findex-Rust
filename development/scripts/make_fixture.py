#!/usr/bin/env python3
"""Creates a deterministic folder for UI tests and benchmarks (stdlib only).

usage: make_fixture.py DIR [--big N]   N = extra empty files for a large-folder benchmark
"""
import os
import struct
import sys
import zlib


def png(path, w, h, rgb):
    raw = b"".join(b"\x00" + bytes(rgb) * w for _ in range(h))

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n")
        f.write(chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)))
        f.write(chunk(b"IDAT", zlib.compress(raw)))
        f.write(chunk(b"IEND", b""))


def main():
    root = sys.argv[1]
    os.makedirs(root, exist_ok=True)
    for d in ["Alpha", "Beta", "Gamma/Deep/Deeper", "Projects/findex"]:
        os.makedirs(os.path.join(root, d), exist_ok=True)
    files = {
        "README.md": b"# readme\n",
        "notes.txt": b"hello\n" * 100,
        "report.pdf": b"%PDF-1.4\n" * 10,
        "budget.xlsx": b"x" * 2048,
        "song.mp3": b"\x00" * 4096,
        "movie.mkv": b"\x00" * 8192,
        "archive.zip": b"PK" * 512,
        "main.rs": b"fn main() {}\n",
        "file2.txt": b"2",
        "file10.txt": b"10",
        ".hidden_config": b"secret",
        "Gamma/Deep/Deeper/needle_in_haystack.txt": b"found",
        "Projects/findex/Cargo.toml": b"[package]\n",
    }
    for name, data in files.items():
        with open(os.path.join(root, name), "wb") as f:
            f.write(data)
    png(os.path.join(root, "sunset.png"), 320, 200, (240, 120, 40))
    png(os.path.join(root, "ocean.png"), 200, 320, (30, 110, 200))
    png(os.path.join(root, "forest.png"), 256, 256, (40, 150, 60))
    # 2 GB sparse file: exercises progress, speed graph, pause and resume in the transfer panel.
    with open(os.path.join(root, "Beta", "big.bin"), "wb") as f:
        f.truncate(2048 * 1024 * 1024)
    if "--big" in sys.argv:
        n = int(sys.argv[sys.argv.index("--big") + 1])
        big = os.path.join(root, "huge")
        os.makedirs(big, exist_ok=True)
        for i in range(n):
            open(os.path.join(big, f"file_{i:07d}.dat"), "wb").close()


if __name__ == "__main__":
    main()
