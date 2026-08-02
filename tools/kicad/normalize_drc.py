#!/usr/bin/env python3
"""Validate a KiCad DRC report and emit deterministic CircuitC evidence."""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys
from typing import Any

DRC_SCHEMA = "https://schemas.kicad.org/drc.v1.json"
ERC_SCHEMA = "https://schemas.kicad.org/erc.v1.json"
EXPECTED_INCLUDED_SEVERITIES = ["error", "exclusion", "warning"]
LIBRARY_WARNING_DESCRIPTION = (
    "The current configuration does not include the footprint library 'CircuitC'"
)
LOGICAL_SOURCE_PATTERN = re.compile(r"[A-Za-z_][A-Za-z0-9_-]*\.circuitc\Z")
UUID_V8_PATTERN = re.compile(
    r"[0-9a-f]{8}-[0-9a-f]{4}-8[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}\Z"
)
CANONICAL_TOKEN_PATTERN = re.compile(r"[A-Za-z0-9_+\-./]+\Z")


class ValidationError(Exception):
    pass


def _require_list(report: dict[str, Any], key: str) -> list[Any]:
    value = report.get(key)
    if not isinstance(value, list):
        raise ValidationError(f"KiCad report field {key!r} must be a list")
    return value


def _canonical_items(
    items: list[Any], identity_map: dict[str, dict[str, Any]] | None = None
) -> list[dict[str, Any]]:
    normalized = []
    for item in items:
        if not isinstance(item, dict) or not isinstance(item.get("description"), str):
            raise ValidationError("every KiCad violation item must have a description")
        normalized_item = {key: value for key, value in item.items() if key not in {"path", "file"}}
        if identity_map is not None:
            uuid = normalized_item.get("uuid")
            if not isinstance(uuid, str):
                raise ValidationError(
                    "every KiCad finding item requires a UUID when an identity map is supplied"
                )
            if uuid not in identity_map:
                raise ValidationError(f"KiCad finding UUID {uuid} is absent from the identity map")
            normalized_item["circuitc"] = identity_map[uuid]
        normalized.append(normalized_item)
    return sorted(
        normalized,
        key=lambda value: json.dumps(value, sort_keys=True, separators=(",", ":")),
    )


def _canonical_findings(
    findings: list[Any],
    identity_map: dict[str, dict[str, Any]] | None,
    category: str,
) -> list[dict[str, Any]]:
    normalized = []
    for finding in findings:
        if not isinstance(finding, dict):
            raise ValidationError(f"every KiCad {category} finding must be an object")
        raw_items = finding.get("items")
        if not isinstance(raw_items, list):
            raise ValidationError(f"every KiCad {category} finding must contain an items list")
        normalized_finding = dict(finding)
        normalized_finding["items"] = _canonical_items(raw_items, identity_map)
        normalized.append(normalized_finding)
    return sorted(
        normalized,
        key=lambda value: json.dumps(value, sort_keys=True, separators=(",", ":")),
    )


def _validate_report_policy(
    report: dict[str, Any], allowed_ignored_checks: list[str]
) -> tuple[list[str], list[dict[str, Any]]]:
    included_severities = _require_list(report, "included_severities")
    if not all(isinstance(severity, str) for severity in included_severities):
        raise ValidationError("every included KiCad severity must be a string")
    if sorted(included_severities) != EXPECTED_INCLUDED_SEVERITIES:
        raise ValidationError(
            "KiCad included severities do not match required policy: "
            f"observed {sorted(included_severities)!r}, "
            f"expected {EXPECTED_INCLUDED_SEVERITIES!r}"
        )

    ignored_checks = _require_list(report, "ignored_checks")
    observed_ignored_checks = []
    for ignored in ignored_checks:
        if (
            not isinstance(ignored, dict)
            or not isinstance(ignored.get("key"), str)
            or not isinstance(ignored.get("description"), str)
        ):
            raise ValidationError(
                "every ignored KiCad check must have a stable key and description"
            )
        observed_ignored_checks.append(ignored["key"])
    if sorted(observed_ignored_checks) != sorted(allowed_ignored_checks):
        raise ValidationError(
            "KiCad ignored checks do not match the allowlist: "
            f"observed {sorted(observed_ignored_checks)!r}, "
            f"expected {sorted(allowed_ignored_checks)!r}"
        )
    return (
        sorted(included_severities),
        sorted(ignored_checks, key=lambda item: item["key"]),
    )


def normalize(
    report: dict[str, Any],
    expected_major: int,
    allowed_library_references: list[str],
    allowed_ignored_checks: list[str],
    identity_map: dict[str, dict[str, Any]] | None = None,
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

    normalized_unconnected = _canonical_findings(
        _require_list(report, "unconnected_items"), identity_map, "unconnected"
    )
    normalized_schematic_parity = _canonical_findings(
        _require_list(report, "schematic_parity"),
        identity_map,
        "schematic-parity",
    )
    if normalized_unconnected or normalized_schematic_parity:
        raise ValidationError(
            "unexpected KiCad connectivity findings: "
            + json.dumps(
                {
                    "schematic_parity": normalized_schematic_parity,
                    "unconnected_items": normalized_unconnected,
                },
                sort_keys=True,
                separators=(",", ":"),
            )
        )

    normalized_violations = []
    unexpected_violations = []
    observed_library_references = []
    for violation in _require_list(report, "violations"):
        if not isinstance(violation, dict):
            raise ValidationError("every KiCad violation must be an object")
        raw_items = violation.get("items")
        if not isinstance(raw_items, list):
            raise ValidationError("every KiCad violation must contain an items list")
        items = _canonical_items(raw_items, identity_map)
        is_allowed_library_warning = (
            violation.get("severity") == "warning"
            and violation.get("type") == "lib_footprint_issues"
            and violation.get("description") == LIBRARY_WARNING_DESCRIPTION
        )
        if not is_allowed_library_warning:
            normalized_violation = dict(violation)
            normalized_violation["items"] = items
            unexpected_violations.append(normalized_violation)
            continue
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

    if unexpected_violations:
        raise ValidationError(
            "unexpected KiCad violations: "
            + json.dumps(
                sorted(
                    unexpected_violations,
                    key=lambda value: json.dumps(value, sort_keys=True, separators=(",", ":")),
                ),
                sort_keys=True,
                separators=(",", ":"),
            )
        )

    if sorted(observed_library_references) != sorted(allowed_library_references):
        raise ValidationError(
            "KiCad library-warning references do not match the allowlist: "
            f"observed {sorted(observed_library_references)!r}, "
            f"expected {sorted(allowed_library_references)!r}"
        )

    included_severities, ignored_checks = _validate_report_policy(report, allowed_ignored_checks)

    source = report.get("source")
    if not isinstance(source, str):
        raise ValidationError("KiCad report is missing source")
    coordinate_units = report.get("coordinate_units")
    if not isinstance(coordinate_units, str):
        raise ValidationError("KiCad report is missing coordinate_units")
    return {
        "schema_version": 1,
        "report_kind": "drc",
        "host": {"name": "kicad", "major": major, "version": version},
        "source": pathlib.PurePosixPath(source.replace("\\", "/")).name,
        "coordinate_units": coordinate_units,
        "included_severities": included_severities,
        "ignored_checks": ignored_checks,
        "schematic_parity": [],
        "unconnected_items": [],
        "violations": sorted(
            normalized_violations,
            key=lambda value: json.dumps(value, sort_keys=True, separators=(",", ":")),
        ),
    }


def normalize_erc(
    report: dict[str, Any],
    expected_major: int,
    allowed_ignored_checks: list[str],
    identity_map: dict[str, dict[str, Any]] | None = None,
) -> dict[str, Any]:
    if report.get("$schema") != ERC_SCHEMA:
        raise ValidationError(f"unsupported KiCad ERC schema {report.get('$schema')!r}")

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

    normalized_sheets = []
    unexpected_sheets = []
    for sheet in _require_list(report, "sheets"):
        if not isinstance(sheet, dict):
            raise ValidationError("every KiCad ERC sheet must be an object")
        path = sheet.get("path")
        uuid_path = sheet.get("uuid_path")
        if not isinstance(path, str) or not isinstance(uuid_path, str):
            raise ValidationError("every KiCad ERC sheet requires path and uuid_path")
        raw_violations = sheet.get("violations")
        if not isinstance(raw_violations, list):
            raise ValidationError("every KiCad ERC sheet requires a violations list")
        violations = _canonical_findings(raw_violations, identity_map, "ERC violation")
        normalized_sheet = {
            "path": path,
            "uuid_path": uuid_path,
            "violations": violations,
        }
        normalized_sheets.append(normalized_sheet)
        if violations:
            unexpected_sheets.append(normalized_sheet)

    if unexpected_sheets:
        raise ValidationError(
            "unexpected KiCad ERC violations: "
            + json.dumps(
                sorted(unexpected_sheets, key=lambda sheet: sheet["uuid_path"]),
                sort_keys=True,
                separators=(",", ":"),
            )
        )

    included_severities, ignored_checks = _validate_report_policy(report, allowed_ignored_checks)
    source = report.get("source")
    if not isinstance(source, str):
        raise ValidationError("KiCad report is missing source")
    coordinate_units = report.get("coordinate_units")
    if not isinstance(coordinate_units, str):
        raise ValidationError("KiCad report is missing coordinate_units")
    return {
        "schema_version": 1,
        "report_kind": "erc",
        "host": {"name": "kicad", "major": major, "version": version},
        "source": pathlib.PurePosixPath(source.replace("\\", "/")).name,
        "coordinate_units": coordinate_units,
        "included_severities": included_severities,
        "ignored_checks": ignored_checks,
        "sheets": sorted(normalized_sheets, key=lambda sheet: sheet["uuid_path"]),
    }


def _semantic_path_is_valid(value: str) -> bool:
    return (
        bool(value)
        and not value.startswith(".")
        and not value.endswith(".")
        and all(CANONICAL_TOKEN_PATTERN.fullmatch(part) for part in value.split("."))
    )


def load_identity_map(
    path: pathlib.Path | None,
) -> tuple[dict[str, dict[str, Any]] | None, str | None]:
    if path is None:
        return None, None
    with path.open(encoding="utf-8") as source:
        manifest = json.load(source)
    if (
        not isinstance(manifest, dict)
        or type(manifest.get("schema_version")) is not int
        or manifest["schema_version"] != 1
    ):
        raise ValidationError("unsupported CircuitC KiCad identity map")
    identities = manifest.get("identities")
    if not isinstance(identities, list):
        raise ValidationError("CircuitC KiCad identity map requires an identities list")
    manifest_source = manifest.get("source")
    if not isinstance(manifest_source, str) or not LOGICAL_SOURCE_PATTERN.fullmatch(
        manifest_source
    ):
        raise ValidationError(
            "CircuitC KiCad identity map requires a source string in logical <design>.circuitc basename form"
        )
    if set(manifest) != {"schema_version", "source", "identities"}:
        raise ValidationError(
            "CircuitC KiCad identity map must contain exactly schema_version, source, and identities"
        )
    result = {}
    semantic_paths = set()
    for identity in identities:
        if not isinstance(identity, dict) or set(identity) != {
            "uuid",
            "semantic_path",
            "location",
        }:
            raise ValidationError(
                "every CircuitC KiCad identity must contain exactly uuid, semantic_path, and location"
            )
        uuid = identity.get("uuid")
        semantic_path = identity.get("semantic_path")
        if not isinstance(uuid, str) or not UUID_V8_PATTERN.fullmatch(uuid):
            raise ValidationError("every CircuitC KiCad identity requires a canonical UUIDv8")
        if not isinstance(semantic_path, str) or not _semantic_path_is_valid(semantic_path):
            raise ValidationError(
                "every CircuitC KiCad identity requires a canonical semantic path"
            )
        location = identity.get("location")
        if location is not None:
            if (
                not isinstance(location, dict)
                or set(location) != {"start", "end", "line", "column"}
                or any(
                    type(location.get(field)) is not int
                    for field in ("start", "end", "line", "column")
                )
            ):
                raise ValidationError(
                    "CircuitC KiCad identity locations require exactly integer start, end, line, and column"
                )
            if (
                location["start"] < 0
                or location["end"] < location["start"]
                or location["line"] < 1
                or location["column"] < 1
            ):
                raise ValidationError("CircuitC KiCad identity location is out of range")
        if uuid in result:
            raise ValidationError(f"duplicate CircuitC KiCad UUID {uuid}")
        if semantic_path in semantic_paths:
            raise ValidationError(f"duplicate CircuitC KiCad semantic path {semantic_path}")
        semantic_paths.add(semantic_path)
        result[uuid] = {
            "semantic_path": semantic_path,
            "source": manifest_source,
            "location": location,
        }
    return result, manifest_source


def validate_manifest_source(report: dict[str, Any], manifest_source: str | None) -> None:
    if manifest_source is None:
        return
    report_source = report.get("source")
    if not isinstance(report_source, str):
        raise ValidationError("KiCad report is missing source")
    report_name = pathlib.PurePosixPath(report_source.replace("\\", "/")).name
    report_stem = None
    for suffix in (".kicad_sch", ".kicad_pcb"):
        if report_name.endswith(suffix):
            report_stem = report_name[: -len(suffix)]
            break
    manifest_stem = manifest_source.removesuffix(".circuitc")
    if report_stem != manifest_stem:
        raise ValidationError(
            "CircuitC identity-map source does not match the KiCad report source: "
            f"manifest {manifest_source!r}, report {report_name!r}"
        )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--raw", required=True, type=pathlib.Path)
    parser.add_argument("--normalized", required=True, type=pathlib.Path)
    parser.add_argument("--expected-major", required=True, type=int)
    parser.add_argument("--allow-library-warning", action="append", default=[])
    parser.add_argument("--allow-ignored-check", action="append", default=[])
    parser.add_argument("--identity-map", type=pathlib.Path)
    args = parser.parse_args()

    try:
        with args.raw.open(encoding="utf-8") as source:
            report = json.load(source)
        if not isinstance(report, dict):
            raise ValidationError("KiCad DRC report root must be an object")
        identity_map, manifest_source = load_identity_map(args.identity_map)
        validate_manifest_source(report, manifest_source)
        if report.get("$schema") == ERC_SCHEMA:
            if args.allow_library_warning:
                raise ValidationError("library-warning allowlists apply only to DRC reports")
            normalized = normalize_erc(
                report,
                args.expected_major,
                args.allow_ignored_check,
                identity_map,
            )
        else:
            normalized = normalize(
                report,
                args.expected_major,
                args.allow_library_warning,
                args.allow_ignored_check,
                identity_map,
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
