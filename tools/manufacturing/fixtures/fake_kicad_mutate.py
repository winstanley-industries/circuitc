#!/usr/bin/env python3
"""Test fake that changes the staged board while claiming host success."""

from __future__ import annotations

import pathlib
import sys

if sys.argv[1:] == ["--version"]:
    print("10.0.5")
    raise SystemExit(0)

board = pathlib.Path(sys.argv[-1])
board.chmod(0o600)
board.write_bytes(board.read_bytes() + b"\n")
raise SystemExit(0)
