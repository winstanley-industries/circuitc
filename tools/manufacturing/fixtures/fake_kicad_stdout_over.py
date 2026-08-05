#!/usr/bin/env python3
"""Test fake that exceeds the bounded stdout budget by exactly one byte."""

from __future__ import annotations

import sys

if sys.argv[1:] == ["--version"]:
    print("10.0.5")
    raise SystemExit(0)

sys.stdout.buffer.write(b"X" * 1_048_577)
sys.stdout.buffer.flush()
raise SystemExit(7)
