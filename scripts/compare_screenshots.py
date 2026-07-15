#!/usr/bin/env python3
"""Compare two uncompressed RGBA fixture files with the parity acceptance gate."""

import argparse
import pathlib
import struct
import sys


MAGIC = b"FWRGBA1\n"


def load(path: pathlib.Path) -> tuple[int, int, bytes]:
    try:
        raw = path.read_bytes()
    except FileNotFoundError:
        raise ValueError(f"required screenshot fixture is missing: {path}") from None
    if not raw.startswith(MAGIC) or len(raw) < len(MAGIC) + 8:
        raise ValueError(f"{path}: expected FWRGBA1 RGBA fixture")
    width, height = struct.unpack_from("<II", raw, len(MAGIC))
    pixels = raw[len(MAGIC) + 8 :]
    expected = width * height * 4
    if width == 0 or height == 0 or len(pixels) != expected:
        raise ValueError(f"{path}: invalid dimensions/payload ({width}x{height}, {len(pixels)} bytes)")
    return width, height, pixels


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("reference", type=pathlib.Path)
    parser.add_argument("candidate", type=pathlib.Path)
    parser.add_argument("--max-delta", type=int, default=2)
    parser.add_argument("--minimum-percent", type=float, default=99.5)
    args = parser.parse_args()
    try:
        rw, rh, reference = load(args.reference)
        cw, ch, candidate = load(args.candidate)
        if (rw, rh) != (cw, ch):
            raise ValueError(f"dimension mismatch: reference {rw}x{rh}, candidate {cw}x{ch}")
        passed = sum(
            all(abs(reference[i + channel] - candidate[i + channel]) <= args.max_delta for channel in range(4))
            for i in range(0, len(reference), 4)
        )
        total = rw * rh
        percent = passed * 100.0 / total
        print(f"pixels_within_delta={passed}/{total} ({percent:.6f}%) max_delta={args.max_delta}")
        if percent + 1e-12 < args.minimum_percent:
            raise ValueError(f"pixel parity failed: {percent:.6f}% < {args.minimum_percent:.6f}%")
        return 0
    except ValueError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
