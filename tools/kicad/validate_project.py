#!/usr/bin/env python3
"""Validate the deterministic CircuitC subset of a KiCad project file."""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys
from typing import Any

PROJECT_KEYS = {
    "board",
    "boards",
    "cvpcb",
    "erc",
    "libraries",
    "meta",
    "net_settings",
    "pcbnew",
    "schematic",
    "sheets",
    "text_variables",
}
OBJECT_FIELDS = {
    "board",
    "cvpcb",
    "erc",
    "net_settings",
    "pcbnew",
    "schematic",
    "text_variables",
}
LIST_FIELDS = {"boards", "sheets"}
LIBRARY_KEYS = {"pinned_footprint_libs", "pinned_symbol_libs"}
META_KEYS = {"filename", "version"}
PROJECT_BASENAME = re.compile(r"[A-Za-z_][A-Za-z0-9_-]*\.kicad_pro\Z")


class ValidationError(Exception):
    pass


def validate_project(
    project: Any, expected_filename: str, artifact_filename: str
) -> dict[str, Any]:
    if not PROJECT_BASENAME.fullmatch(expected_filename):
        raise ValidationError(
            "expected KiCad project filename must be a canonical .kicad_pro basename"
        )
    if artifact_filename != expected_filename:
        raise ValidationError(
            "KiCad project path basename does not match the expected artifact filename: "
            f"observed {artifact_filename!r}, expected {expected_filename!r}"
        )
    if not isinstance(project, dict):
        raise ValidationError("KiCad project root must be an object")
    if set(project) != PROJECT_KEYS:
        raise ValidationError(
            "KiCad project fields do not match the CircuitC project contract: "
            f"observed {sorted(project)!r}, expected {sorted(PROJECT_KEYS)!r}"
        )

    for field in sorted(OBJECT_FIELDS):
        if project[field] != {}:
            raise ValidationError(f"KiCad project field {field!r} must be an empty object")
    for field in sorted(LIST_FIELDS):
        if project[field] != []:
            raise ValidationError(f"KiCad project field {field!r} must be an empty list")

    libraries = project["libraries"]
    if not isinstance(libraries, dict) or set(libraries) != LIBRARY_KEYS:
        raise ValidationError(
            "KiCad project libraries must contain pinned footprint and symbol lists"
        )
    for field in sorted(LIBRARY_KEYS):
        value = libraries[field]
        if value != []:
            raise ValidationError(f"KiCad project library field {field!r} must be an empty list")

    meta = project["meta"]
    if not isinstance(meta, dict) or set(meta) != META_KEYS:
        raise ValidationError("KiCad project meta must contain filename and version")
    if meta["filename"] != expected_filename:
        raise ValidationError(
            "KiCad project filename does not match its artifact: "
            f"observed {meta['filename']!r}, expected {expected_filename!r}"
        )
    if type(meta["version"]) is not int or meta["version"] != 1:
        raise ValidationError("KiCad project meta version must be integer 1")

    return {
        "schema_version": 1,
        "artifact_kind": "kicad_project",
        "filename": expected_filename,
        "meta_version": meta["version"],
        "project_fields": sorted(project),
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--project", required=True, type=pathlib.Path)
    parser.add_argument("--expected-filename", required=True)
    parser.add_argument("--normalized", required=True, type=pathlib.Path)
    args = parser.parse_args()

    try:
        with args.project.open(encoding="utf-8") as source:
            project = json.load(source)
        normalized = validate_project(project, args.expected_filename, args.project.name)
    except (OSError, json.JSONDecodeError, ValidationError) as error:
        print(f"CircuitC KiCad project validation failed: {error}", file=sys.stderr)
        return 1

    args.normalized.parent.mkdir(parents=True, exist_ok=True)
    with args.normalized.open("w", encoding="utf-8", newline="\n") as output:
        json.dump(normalized, output, indent=2, sort_keys=True)
        output.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
