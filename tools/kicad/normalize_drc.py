#!/usr/bin/env python3
"""Validate a KiCad DRC report and emit deterministic CircuitC evidence."""

import argparse
import json
import pathlib
import sys
from typing import Any


DRC_SCHEMA = "https://schemas.kicad.org/drc.v1.json"
LIBRARY_WARNING_DESCRIPTION = (
    "The current configuration does not include the footprint library 'CircuitC'"
)


class ValidationError(Exception):
    pass


def _require_list(report: dict[str, Any], key: str) -> list[Any]:
    value = report.get(key)
    if not isinstance(value, list):
        raise ValidationError(f"KiCad report field {key!r} must be a list")
    return value


def _canonical_items(items: list[Any]) -> list[dict[str, Any]]:
    normalized = []
    for item in items:
        if not isinstance(item, dict) or not isinstance(item.get("description"), str):
            raise ValidationError("every KiCad violation item must have a description")
        normalized_item = {
            key: value
            for key, value in item.items()
            if key not in {"path", "file"}
        }
        normalized.append(normalized_item)
    return sorted(
        normalized,
        key=lambda value: json.dumps(value, sort_keys=True, separators=(",", ":")),
    )


def normalize(
    report: dict[str, Any], expected_major: int, allowed_library_references: list[str]
) -> dict[str, Any]:
    if report.get("$schema") != DRC_SCHEMA:
        raise ValidationError(f"unsupported KiCad DRC schema {report.get('$schema')!r}")

    version = report.get("kicad_version")
    if not isinstance(version, str):
        raise ValidationError("KiCad report is missing kicad_version")
    try:
        major = int(version.split(".", 1)[0])
    except ValueError as error:
        raise ValidationError(f"invalid KiCad version {version!r}") from error
    if major != expected_major:
        raise ValidationError(
            f"KiCad major version {major} does not match supported version {expected_major}"
        )

    unconnected = _require_list(report, "unconnected_items")
    if unconnected:
        raise ValidationError(f"KiCad reported {len(unconnected)} unconnected item(s)")
    schematic_parity = _require_list(report, "schematic_parity")
    if schematic_parity:
        raise ValidationError(
            f"KiCad reported {len(schematic_parity)} schematic parity issue(s)"
        )

    normalized_violations = []
    observed_library_references = []
    for violation in _require_list(report, "violations"):
        if not isinstance(violation, dict):
            raise ValidationError("every KiCad violation must be an object")
        raw_items = violation.get("items")
        if not isinstance(raw_items, list):
            raise ValidationError("every KiCad violation must contain an items list")
        items = _canonical_items(raw_items)
        is_allowed_library_warning = (
            violation.get("severity") == "warning"
            and violation.get("type") == "lib_footprint_issues"
            and violation.get("description") == LIBRARY_WARNING_DESCRIPTION
        )
        if not is_allowed_library_warning:
            raise ValidationError(
                "unexpected KiCad violation: "
                + json.dumps(violation, sort_keys=True, separators=(",", ":"))
            )
        for item in items:
            description = item["description"]
            prefix = "Footprint "
            if not description.startswith(prefix):
                raise ValidationError(
                    f"unexpected library-warning item description {description!r}"
                )
            observed_library_references.append(description[len(prefix) :])
        normalized_violations.append(
            {
                "description": violation["description"],
                "items": items,
                "severity": violation["severity"],
                "type": violation["type"],
            }
        )

    if sorted(observed_library_references) != sorted(allowed_library_references):
        raise ValidationError(
            "KiCad library-warning references do not match the allowlist: "
            f"observed {sorted(observed_library_references)!r}, "
            f"expected {sorted(allowed_library_references)!r}"
        )

    ignored_checks = _require_list(report, "ignored_checks")
    for ignored in ignored_checks:
        if not isinstance(ignored, dict) or not isinstance(ignored.get("key"), str):
            raise ValidationError("every ignored KiCad check must have a stable key")

    source = report.get("source")
    if not isinstance(source, str):
        raise ValidationError("KiCad report is missing source")
    coordinate_units = report.get("coordinate_units")
    if not isinstance(coordinate_units, str):
        raise ValidationError("KiCad report is missing coordinate_units")
    included_severities = _require_list(report, "included_severities")
    if not all(isinstance(severity, str) for severity in included_severities):
        raise ValidationError("every included KiCad severity must be a string")

    return {
        "schema_version": 1,
        "host": {"name": "kicad", "major": major, "version": version},
        "source": pathlib.PurePosixPath(source.replace("\\", "/")).name,
        "coordinate_units": coordinate_units,
        "included_severities": sorted(included_severities),
        "ignored_checks": sorted(ignored_checks, key=lambda item: item["key"]),
        "schematic_parity": [],
        "unconnected_items": [],
        "violations": sorted(
            normalized_violations,
            key=lambda value: json.dumps(
                value, sort_keys=True, separators=(",", ":")
            ),
        ),
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--raw", required=True, type=pathlib.Path)
    parser.add_argument("--normalized", required=True, type=pathlib.Path)
    parser.add_argument("--expected-major", required=True, type=int)
    parser.add_argument("--allow-library-warning", action="append", default=[])
    args = parser.parse_args()

    try:
        with args.raw.open(encoding="utf-8") as source:
            report = json.load(source)
        if not isinstance(report, dict):
            raise ValidationError("KiCad DRC report root must be an object")
        normalized = normalize(
            report, args.expected_major, args.allow_library_warning
        )
    except (OSError, json.JSONDecodeError, ValidationError) as error:
        print(f"CircuitC KiCad validation failed: {error}", file=sys.stderr)
        return 1

    args.normalized.parent.mkdir(parents=True, exist_ok=True)
    with args.normalized.open("w", encoding="utf-8", newline="\n") as output:
        json.dump(normalized, output, indent=2, sort_keys=True)
        output.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
