#!/usr/bin/env python3
"""Validate a FreeWheeling real-time performance result JSON document."""

import argparse
import json
import pathlib
import sys


REQUIRED = {
    "schema_version": int,
    "sample_rate_hz": int,
    "buffer_frames": int,
    "duration_seconds": (int, float),
    "callback_p99_us": (int, float),
    "callback_deadline_us": (int, float),
    "callback_allocations": int,
    "blocking_lock_attempts": int,
    "unexplained_xruns": int,
    "rss_start_bytes": int,
    "rss_peak_bytes": int,
}


def fail(message: str) -> None:
    raise ValueError(message)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("result", type=pathlib.Path)
    parser.add_argument("--require-stress", action="store_true", help="require the two-hour acceptance duration")
    args = parser.parse_args()
    try:
        try:
            value = json.loads(args.result.read_text(encoding="utf-8"))
        except FileNotFoundError:
            fail(f"required performance result is missing: {args.result}")
        except json.JSONDecodeError as error:
            fail(f"invalid JSON: {error}")
        if not isinstance(value, dict):
            fail("result must be a JSON object")
        for name, expected_type in REQUIRED.items():
            if name not in value:
                fail(f"missing required field: {name}")
            if isinstance(value[name], bool) or not isinstance(value[name], expected_type):
                fail(f"field {name} has the wrong type")
            if value[name] < 0:
                fail(f"field {name} must be non-negative")
        if value["schema_version"] != 1:
            fail("schema_version must be 1")
        if value["sample_rate_hz"] != 48000 or value["buffer_frames"] not in (128, 256):
            fail("acceptance requires 48000 Hz and 128 or 256 frames")
        if value["callback_allocations"] or value["blocking_lock_attempts"] or value["unexplained_xruns"]:
            fail("callback allocations, blocking locks, and unexplained xruns must all be zero")
        if value["callback_p99_us"] >= value["callback_deadline_us"] * 0.70:
            fail("callback p99 must be below 70% of the deadline")
        if value["rss_peak_bytes"] < value["rss_start_bytes"]:
            fail("rss_peak_bytes must be at least rss_start_bytes")
        if args.require_stress and value["duration_seconds"] < 7200:
            fail("stress acceptance requires at least 7200 seconds")
        print("performance result valid")
        return 0
    except (OSError, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
