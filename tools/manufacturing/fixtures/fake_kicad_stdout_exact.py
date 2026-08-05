#!/usr/bin/env python3
"""Test fake that reaches the exact stdout budget, then reports host failure."""

from __future__ import annotations

import sys

if sys.argv[1:] == ["--version"]:
    print("10.0.5")
    raise SystemExit(0)

sys.stdout.buffer.write(b"X" * 1_048_576)
sys.stdout.buffer.flush()
raise SystemExit(7)
