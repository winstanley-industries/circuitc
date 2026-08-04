#!/usr/bin/env python3
import sys

if sys.argv[1:] == ["--version"]:
    print("10.0.5")
    raise SystemExit(0)
raise SystemExit(2)
