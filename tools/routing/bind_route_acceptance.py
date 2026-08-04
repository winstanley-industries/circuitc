#!/usr/bin/env python3
"""Bind authenticated APGAR routing evidence to clean KiCad host reports."""

from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import re
import stat
import sys
from typing import Any

MAX_INPUT_BYTES = 64 * 1024 * 1024
SHA256_PATTERN = re.compile(r"[0-9a-f]{64}\Z")
CANDIDATE_ID_PATTERN = re.compile(r"[0-9a-f]{32}\Z")
SIGNATURE_PATTERN = re.compile(r"[0-9a-f]{32}\Z")
CHECKSUM_PATTERN = re.compile(r"[0-9a-f]{16}\Z")
UUID_V8_PATTERN = re.compile(
    r"[0-9a-f]{8}-[0-9a-f]{4}-8[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}\Z"
)


class AcceptanceError(Exception):
    pass


def _reject_duplicate_keys(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise AcceptanceError(f"duplicate JSON key {key!r}")
        result[key] = value
    return result


def _reject_constant(value: str) -> None:
    raise AcceptanceError(f"non-finite JSON number {value!r}")


def _read_regular(path: pathlib.Path) -> bytes:
    metadata = path.lstat()
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_size > MAX_INPUT_BYTES:
        raise AcceptanceError(f"{path} is not a bounded regular file")
    data = path.read_bytes()
    if len(data) != metadata.st_size:
        raise AcceptanceError(f"{path} changed while it was read")
    return data


def _load_json(path: pathlib.Path) -> tuple[dict[str, Any], bytes]:
    data = _read_regular(path)
    try:
        text = data.decode("utf-8")
        value = json.loads(
            text,
            object_pairs_hook=_reject_duplicate_keys,
            parse_constant=_reject_constant,
        )
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise AcceptanceError(f"{path} is not strict UTF-8 JSON: {error}") from error
    if not isinstance(value, dict):
        raise AcceptanceError(f"{path} JSON root must be an object")
    return value, data


def _load_canonical_contract(path: pathlib.Path) -> tuple[dict[str, Any], bytes]:
    value, data = _load_json(path)
    canonical = (
        json.dumps(value, ensure_ascii=False, separators=(",", ":"), allow_nan=False) + "\n"
    ).encode()
    if canonical != data:
        raise AcceptanceError(f"{path} is not canonical compact JSON with one final LF")
    return value, data


def _require_keys(value: Any, keys: set[str], path: str) -> dict[str, Any]:
    if not isinstance(value, dict) or set(value) != keys:
        raise AcceptanceError(f"{path} must contain exactly {sorted(keys)!r}")
    return value


def _require_list(value: Any, path: str) -> list[Any]:
    if not isinstance(value, list):
        raise AcceptanceError(f"{path} must be a list")
    return value


def _require_string(value: Any, path: str) -> str:
    if not isinstance(value, str) or not value:
        raise AcceptanceError(f"{path} must be a non-empty string")
    return value


def _require_int(value: Any, path: str) -> int:
    if type(value) is not int:
        raise AcceptanceError(f"{path} must be an integer")
    return value


def _require_digest(value: Any, pattern: re.Pattern[str], path: str) -> str:
    value = _require_string(value, path)
    if not pattern.fullmatch(value):
        raise AcceptanceError(f"{path} has a noncanonical digest")
    return value


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _require_schema(value: dict[str, Any], name: str, path: str) -> None:
    if value.get("schema_name") != name or value.get("schema_version") != 1:
        raise AcceptanceError(f"{path} has an unsupported schema")


REQUEST_KEYS = {
    "schema_name",
    "schema_version",
    "design_name",
    "design_fingerprint_sha256",
    "request_path",
    "request_identity_sha256",
    "expected_apgar_source_revision",
    "expected_apgar_contract_identity",
    "dbu_per_millimeter",
    "board_revision",
    "adapter_name",
    "adapter_version",
    "layers",
    "nets",
    "terminals",
    "obstacles",
    "routing_profile",
    "compiler_profile",
    "planar_route",
    "resource_limits",
    "unsupported_host_rules",
}
RESULT_KEYS = {
    "schema_name",
    "schema_version",
    "request_sha256",
    "request_path",
    "tool",
    "replay",
    "outcome",
}
PROJECTION_KEYS = {
    "schema_name",
    "schema_version",
    "design_name",
    "request_path",
    "request_identity_sha256",
    "request_sha256",
    "result_sha256",
    "selected_candidate_id",
    "candidate_geometry_signature",
    "candidate_resource_signature",
    "candidate_payload_checksum",
    "tool",
    "segments",
    "kicad_pcb_sha256",
}
TOOL_KEYS = {
    "name",
    "version",
    "contract_identity",
    "source_revision",
    "executable_sha256",
    "device_class",
}
CANDIDATE_KEYS = {
    "schema_major",
    "schema_minor",
    "id",
    "net",
    "intended_terminals",
    "associations",
    "geometry_schema_version",
    "resource_schema_version",
    "policy",
    "policy_identity",
    "provenance",
    "geometry",
    "resources",
    "metrics",
    "constraints",
    "geometry_signature",
    "resource_signature",
    "payload_checksum",
    "logical_bytes",
}


def _validate_host_report(
    report: dict[str, Any], kind: str, expected_source: str
) -> dict[str, Any]:
    common = {
        "schema_version",
        "report_kind",
        "host",
        "source",
        "coordinate_units",
        "included_severities",
        "ignored_checks",
    }
    keys = (
        common | {"schematic_parity", "unconnected_items", "violations"}
        if kind == "drc"
        else common | {"sheets"}
    )
    _require_keys(report, keys, kind)
    if report["schema_version"] != 1 or report["report_kind"] != kind:
        raise AcceptanceError(f"{kind} report has an unsupported schema")
    host = _require_keys(report["host"], {"name", "major", "version"}, f"{kind}.host")
    if (
        host["name"] != "kicad"
        or host["major"] != 10
        or not isinstance(host["version"], str)
        or not host["version"].startswith("10.")
    ):
        raise AcceptanceError(f"{kind} report is not supported KiCad 10 evidence")
    if report["source"] != expected_source:
        raise AcceptanceError(f"{kind} report source does not match {expected_source}")
    _require_string(report["coordinate_units"], f"{kind}.coordinate_units")
    if report["included_severities"] != ["error", "exclusion", "warning"]:
        raise AcceptanceError(f"{kind}.included_severities does not match required policy")
    ignored_checks = _require_list(report["ignored_checks"], f"{kind}.ignored_checks")
    for index, ignored in enumerate(ignored_checks):
        ignored = _require_keys(ignored, {"description", "key"}, f"{kind}.ignored_checks[{index}]")
        _require_string(ignored["description"], f"{kind}.ignored_checks[{index}].description")
        _require_string(ignored["key"], f"{kind}.ignored_checks[{index}].key")
    if kind == "drc":
        for field in ("schematic_parity", "unconnected_items", "violations"):
            if report[field] != []:
                raise AcceptanceError(f"drc.{field} must be empty")
    else:
        sheets = _require_list(report["sheets"], "erc.sheets")
        if not sheets:
            raise AcceptanceError("erc.sheets must not be empty")
        for index, sheet in enumerate(sheets):
            sheet = _require_keys(
                sheet, {"path", "uuid_path", "violations"}, f"erc.sheets[{index}]"
            )
            if sheet["violations"] != []:
                raise AcceptanceError(f"erc.sheets[{index}].violations must be empty")
    return host


def _selected_candidate(result: dict[str, Any]) -> tuple[str, dict[str, Any]]:
    outcome = _require_keys(
        result["outcome"],
        {"kind", "selected_candidate_id", "candidates"},
        "result.outcome",
    )
    if outcome["kind"] != "completed":
        raise AcceptanceError("APGAR outcome is not completed")
    selected_id = _require_digest(
        outcome["selected_candidate_id"], CANDIDATE_ID_PATTERN, "selected_candidate_id"
    )
    candidates = _require_list(outcome["candidates"], "result.outcome.candidates")
    selected = [
        candidate
        for candidate in candidates
        if isinstance(candidate, dict) and candidate.get("id") == selected_id
    ]
    if len(selected) != 1:
        raise AcceptanceError("selected APGAR candidate is missing or duplicated")
    return selected_id, _require_keys(selected[0], CANDIDATE_KEYS, "selected_candidate")


def _validate_apgar_join(
    request: dict[str, Any],
    request_data: bytes,
    result: dict[str, Any],
    result_data: bytes,
    projection: dict[str, Any],
    projection_data: bytes,
) -> tuple[str, dict[str, Any], dict[str, Any]]:
    _require_keys(request, REQUEST_KEYS, "request")
    _require_keys(result, RESULT_KEYS, "result")
    _require_keys(projection, PROJECTION_KEYS, "projection")
    _require_schema(request, "circuitc.apgar_route_request", "request")
    _require_schema(result, "circuitc.apgar_route_result", "result")
    _require_schema(projection, "circuitc.apgar_route_projection", "projection")

    request_sha = _sha256(request_data)
    result_sha = _sha256(result_data)
    if result["request_sha256"] != request_sha or projection["request_sha256"] != request_sha:
        raise AcceptanceError("request digest chain does not match exact request bytes")
    if projection["result_sha256"] != result_sha:
        raise AcceptanceError("result digest chain does not match exact result bytes")
    if (
        result["request_path"] != request["request_path"]
        or projection["request_path"] != request["request_path"]
    ):
        raise AcceptanceError("routing semantic paths do not match")
    if projection["design_name"] != request["design_name"]:
        raise AcceptanceError("projection design name does not match request")

    request_identity = _require_digest(
        request["request_identity_sha256"], SHA256_PATTERN, "request_identity_sha256"
    )
    replay = _require_keys(
        result["replay"],
        {
            "design_fingerprint_sha256",
            "request_identity_sha256",
            "board_revision",
            "deterministic_seed",
            "batch_identity",
            "query_identity",
        },
        "result.replay",
    )
    planar_route = _require_keys(
        request["planar_route"],
        {"net", "start", "goal", "start_layer", "goal_layer", "candidate_policy", "scheduling"},
        "request.planar_route",
    )
    scheduling = _require_keys(
        planar_route["scheduling"],
        {"batch_identity", "query_identity"},
        "request.planar_route.scheduling",
    )
    policy = _require_keys(
        planar_route["candidate_policy"],
        {
            "schema_version",
            "objective",
            "deterministic_seed",
            "candidate_ordinal",
            "orthogonal_step_surcharge",
            "diagonal_step_surcharge",
            "bend_surcharge",
            "banned_resources",
            "resource_penalties",
        },
        "request.planar_route.candidate_policy",
    )
    if (
        replay["design_fingerprint_sha256"] != request["design_fingerprint_sha256"]
        or replay["request_identity_sha256"] != request_identity
        or replay["board_revision"] != request["board_revision"]
        or replay["deterministic_seed"] != policy.get("deterministic_seed")
        or replay["batch_identity"] != scheduling["batch_identity"]
        or replay["query_identity"] != scheduling["query_identity"]
        or projection["request_identity_sha256"] != request_identity
    ):
        raise AcceptanceError("APGAR replay identity does not match the request")

    tool = _require_keys(result["tool"], TOOL_KEYS, "result.tool")
    _require_digest(tool["executable_sha256"], SHA256_PATTERN, "result.tool.executable_sha256")
    if (
        tool["contract_identity"] != request["expected_apgar_contract_identity"]
        or tool["source_revision"] != request["expected_apgar_source_revision"]
        or projection["tool"] != tool
    ):
        raise AcceptanceError("APGAR tool identity does not match the request and projection")

    selected_id, candidate = _selected_candidate(result)
    if projection["selected_candidate_id"] != selected_id:
        raise AcceptanceError("projection does not name the selected APGAR candidate")
    constraints = _require_keys(
        candidate["constraints"],
        {
            "supported_hard_constraints_satisfied",
            "unsupported_rules_remain",
            "connected_intended_terminal_count",
            "exact_validation_status",
        },
        "selected_candidate.constraints",
    )
    if constraints != {
        "supported_hard_constraints_satisfied": True,
        "unsupported_rules_remain": False,
        "connected_intended_terminal_count": 2,
        "exact_validation_status": "passed",
    }:
        raise AcceptanceError("selected candidate lacks exact APGAR admission")
    if candidate["schema_major"] != 1 or candidate["geometry_schema_version"] != 1:
        raise AcceptanceError("selected candidate has an unsupported schema")
    if candidate["policy"] != policy or candidate["net"] != planar_route["net"]:
        raise AcceptanceError("selected candidate policy or net does not match the request")
    provenance = _require_keys(
        candidate["provenance"],
        {
            "generator",
            "generator_version",
            "backend",
            "supported_device_class",
            "deterministic_seed",
            "batch_identity",
            "query_identity",
            "candidate_ordinal",
        },
        "selected_candidate.provenance",
    )
    if (
        provenance.get("backend") != "cpu"
        or provenance.get("supported_device_class") != tool["device_class"]
        or provenance.get("deterministic_seed") != replay["deterministic_seed"]
        or provenance.get("batch_identity") != replay["batch_identity"]
        or provenance.get("query_identity") != replay["query_identity"]
        or provenance.get("candidate_ordinal") != policy.get("candidate_ordinal")
    ):
        raise AcceptanceError("selected candidate provenance does not match replay identity")

    for field, pattern in (
        ("geometry_signature", SIGNATURE_PATTERN),
        ("resource_signature", SIGNATURE_PATTERN),
        ("payload_checksum", CHECKSUM_PATTERN),
    ):
        candidate_value = _require_digest(candidate[field], pattern, f"candidate.{field}")
        if projection[f"candidate_{field}"] != candidate_value:
            raise AcceptanceError(f"projection {field} does not match selected candidate")

    expected_host_rules = [
        {"code": f"CC-ROUTE-HOST-00{index}", "path": f"{request['request_path']}.host.{suffix}"}
        for index, suffix in enumerate(
            (
                "board_edge_clearance",
                "courtyard_clearance",
                "kicad_custom_rules",
                "schematic_board_parity",
            ),
            start=1,
        )
    ]
    if request["unsupported_host_rules"] != expected_host_rules:
        raise AcceptanceError("request does not declare the exact KiCad host-rule boundary")
    return selected_id, candidate, tool


def _validate_projection_geometry(
    request: dict[str, Any],
    candidate: dict[str, Any],
    projection: dict[str, Any],
    pcb_data: bytes,
) -> None:
    layers = {}
    for index, layer in enumerate(_require_list(request["layers"], "request.layers")):
        if not isinstance(layer, dict):
            raise AcceptanceError(f"request.layers[{index}] must be an object")
        routing_id = _require_int(layer.get("routing_id"), f"request.layers[{index}].routing_id")
        side = layer.get("side")
        if side not in {"front", "back"}:
            raise AcceptanceError(f"request.layers[{index}].side is unsupported")
        layers[routing_id] = side
    nets = {}
    for index, net in enumerate(_require_list(request["nets"], "request.nets")):
        if not isinstance(net, dict):
            raise AcceptanceError(f"request.nets[{index}] must be an object")
        nets[json.dumps(net.get("reference"), sort_keys=True, separators=(",", ":"))] = net.get(
            "name"
        )
    net_name = nets.get(json.dumps(candidate["net"], sort_keys=True, separators=(",", ":")))
    if not isinstance(net_name, str):
        raise AcceptanceError("selected candidate net is absent from request catalogue")
    geometry = _require_list(candidate["geometry"], "selected_candidate.geometry")
    segments = _require_list(projection["segments"], "projection.segments")
    if len(geometry) != len(segments) or not geometry:
        raise AcceptanceError("projection is not one-to-one with candidate geometry")
    for index, (primitive, segment) in enumerate(zip(geometry, segments)):
        primitive = _require_keys(
            primitive, {"layer", "start", "end", "width_dbu"}, f"candidate.geometry[{index}]"
        )
        segment = _require_keys(
            segment,
            {
                "ordinal",
                "semantic_path",
                "kicad_uuid",
                "net",
                "layer",
                "start_nm",
                "end_nm",
                "width_nm",
            },
            f"projection.segments[{index}]",
        )
        start = _require_keys(primitive["start"], {"x", "y"}, f"candidate.geometry[{index}].start")
        end = _require_keys(primitive["end"], {"x", "y"}, f"candidate.geometry[{index}].end")
        values = [
            start["x"],
            start["y"],
            end["x"],
            end["y"],
            primitive["width_dbu"],
        ]
        if any(type(value) is not int or value % 2 for value in values):
            raise AcceptanceError(
                "candidate geometry is not losslessly representable in nanometres"
            )
        expected = {
            "ordinal": index,
            "semantic_path": f"{request['request_path']}.segment.{index:08d}",
            "kicad_uuid": segment["kicad_uuid"],
            "net": net_name,
            "layer": layers.get(primitive["layer"]),
            "start_nm": {"x": values[0] // 2, "y": values[1] // 2},
            "end_nm": {"x": values[2] // 2, "y": values[3] // 2},
            "width_nm": values[4] // 2,
        }
        if segment != expected:
            raise AcceptanceError(f"projection segment {index} does not match candidate geometry")
        uuid = _require_string(segment["kicad_uuid"], f"projection.segments[{index}].kicad_uuid")
        if not UUID_V8_PATTERN.fullmatch(uuid) or pcb_data.count(uuid.encode()) != 1:
            raise AcceptanceError(f"projected KiCad UUID {uuid!r} is not unique in exact PCB bytes")


def bind(args: argparse.Namespace) -> bytes:
    request, request_data = _load_canonical_contract(args.request)
    result, result_data = _load_canonical_contract(args.result)
    projection, projection_data = _load_canonical_contract(args.projection)
    pcb_data = _read_regular(args.pcb)
    drc, drc_data = _load_json(args.drc)
    erc, erc_data = _load_json(args.erc)

    selected_id, candidate, tool = _validate_apgar_join(
        request, request_data, result, result_data, projection, projection_data
    )
    pcb_sha = _sha256(pcb_data)
    if projection["kicad_pcb_sha256"] != pcb_sha:
        raise AcceptanceError("projection PCB digest does not match exact emitted board bytes")
    _validate_projection_geometry(request, candidate, projection, pcb_data)

    design_name = _require_string(request["design_name"], "request.design_name")
    drc_host = _validate_host_report(drc, "drc", f"{design_name}.kicad_pcb")
    erc_host = _validate_host_report(erc, "erc", f"{design_name}.kicad_sch")
    if drc_host != erc_host:
        raise AcceptanceError("ERC and DRC host identities do not match")

    manifest = {
        "schema_name": "circuitc.apgar_route_acceptance",
        "schema_version": 1,
        "design_name": design_name,
        "request_path": request["request_path"],
        "request_identity_sha256": request["request_identity_sha256"],
        "request_sha256": _sha256(request_data),
        "result_sha256": _sha256(result_data),
        "projection_sha256": _sha256(projection_data),
        "selected_candidate_id": selected_id,
        "candidate": {
            "geometry_signature": candidate["geometry_signature"],
            "resource_signature": candidate["resource_signature"],
            "payload_checksum": candidate["payload_checksum"],
        },
        "tool": tool,
        "authorities": {
            "apgar_exact_admission": True,
            "kicad_erc_clean": True,
            "kicad_drc_clean": True,
            "kicad_schematic_parity_clean": True,
            "kicad_unconnected_clean": True,
        },
        "kicad": {
            "host": drc_host,
            "pcb_filename": args.pcb.name,
            "pcb_sha256": pcb_sha,
            "drc_filename": args.drc.name,
            "drc_sha256": _sha256(drc_data),
            "erc_filename": args.erc.name,
            "erc_sha256": _sha256(erc_data),
        },
    }
    return (
        json.dumps(manifest, ensure_ascii=False, sort_keys=True, separators=(",", ":")) + "\n"
    ).encode()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--request", required=True, type=pathlib.Path)
    parser.add_argument("--result", required=True, type=pathlib.Path)
    parser.add_argument("--projection", required=True, type=pathlib.Path)
    parser.add_argument("--pcb", required=True, type=pathlib.Path)
    parser.add_argument("--drc", required=True, type=pathlib.Path)
    parser.add_argument("--erc", required=True, type=pathlib.Path)
    parser.add_argument("--output", required=True, type=pathlib.Path)
    args = parser.parse_args()
    try:
        evidence = bind(args)
        if args.output.exists():
            raise AcceptanceError("acceptance output already exists")
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_bytes(evidence)
    except (OSError, AcceptanceError) as error:
        print(f"CircuitC route acceptance failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
