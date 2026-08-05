#!/usr/bin/env python3
import argparse
import hashlib
import json
import pathlib

DOMAIN = b"CIRCUITC-BOARD-ANALYSIS-IDENTITY-V1\0"


def binding(path: str, source: pathlib.Path) -> dict[str, object]:
    data = source.read_bytes()
    return {
        "path": path,
        "byte_length": len(data),
        "sha256": hashlib.sha256(data).hexdigest(),
    }


parser = argparse.ArgumentParser()
parser.add_argument("--project", required=True, type=pathlib.Path)
parser.add_argument("--fabrication-manifest", required=True, type=pathlib.Path)
parser.add_argument("--output", required=True, type=pathlib.Path)
args = parser.parse_args()

preimage = {
    "design_name": "voltage_divider",
    "analysis_path": "release.manufacturability",
    "adapter": "kicad",
    "expected_major": 10,
    "expected_version": "10.0.5",
    "assertions": [
        {
            "assertion_path": "release.manufacturability.erc",
            "capability": "erc_clean",
        },
        {
            "assertion_path": "release.manufacturability.drc",
            "capability": "drc_clean",
        },
        {
            "assertion_path": "release.manufacturability.unconnected",
            "capability": "unconnected_clean",
        },
        {
            "assertion_path": "release.manufacturability.parity",
            "capability": "schematic_parity_clean",
        },
        {
            "assertion_path": "release.manufacturability.fabrication",
            "capability": "fabrication_inventory_complete",
        },
    ],
    "kicad_schematic": binding(
        "voltage_divider.kicad_sch", args.project / "voltage_divider.kicad_sch"
    ),
    "kicad_pcb": binding("voltage_divider.kicad_pcb", args.project / "voltage_divider.kicad_pcb"),
    "kicad_identity_map": binding(
        "voltage_divider.kicad-map.json",
        args.project / "voltage_divider.kicad-map.json",
    ),
    "expected_sheets": [
        {
            "path": "/",
            "uuid_path": "/00000000-0000-8000-8000-000000000000",
        }
    ],
    "project_support": [
        binding(relative, args.project / relative)
        for relative in sorted(
            [
                "voltage_divider.kicad_pro",
                "CircuitC.kicad_sym",
                "CircuitC.pretty/R_0603_1608Metric.kicad_mod",
                "sym-lib-table",
                "fp-lib-table",
            ]
        )
    ],
    "fabrication_request": binding(
        "fabrication/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/request.json",
        args.fabrication_manifest,
    ),
    "fabrication_manifest": binding(
        "fabrication/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/manifest.json",
        args.fabrication_manifest,
    ),
    "policy": {
        "included_severities": ["error", "exclusion", "warning"],
        "erc_ignored_checks": [
            "footprint_filter",
            "four_way_junction",
            "simulation_model_issue",
            "single_global_label",
        ],
        "drc_ignored_checks": [
            "footprint_filters_mismatch",
            "footprint_type_mismatch",
            "missing_courtyard",
            "track_not_centered_on_via",
            "tuning_profile_track_geometries",
        ],
        "drc_library_warning": "The current configuration does not include the footprint library 'CircuitC'",
    },
    "resources": {
        "timeout_ms": 120000,
        "stdout_bytes": 1048576,
        "stderr_bytes": 1048576,
        "file_bytes": 67108864,
        "aggregate_bytes": 268435456,
        "primary_rows": 10000,
        "diagnostics": 256,
    },
    "outputs": [
        {"role": "erc", "path": "erc.normalized.json"},
        {"role": "drc", "path": "drc.normalized.json"},
        {"role": "receipt", "path": "receipt.json"},
    ],
}
identity = hashlib.sha256(DOMAIN + json.dumps(preimage, separators=(",", ":")).encode()).hexdigest()
request = {
    "schema_name": "circuitc.board_analysis_request",
    "schema_version": 1,
    "design_name": preimage["design_name"],
    "analysis_path": preimage["analysis_path"],
    "adapter": preimage["adapter"],
    "expected_major": preimage["expected_major"],
    "expected_version": preimage["expected_version"],
    "analysis_identity_sha256": identity,
    "assertions": preimage["assertions"],
    "kicad_schematic": preimage["kicad_schematic"],
    "kicad_pcb": preimage["kicad_pcb"],
    "kicad_identity_map": preimage["kicad_identity_map"],
    "expected_sheets": preimage["expected_sheets"],
    "project_support": preimage["project_support"],
    "fabrication_request": preimage["fabrication_request"],
    "fabrication_manifest": preimage["fabrication_manifest"],
    "policy": preimage["policy"],
    "resources": preimage["resources"],
    "outputs": preimage["outputs"],
}
args.output.write_text(json.dumps(request, separators=(",", ":")) + "\n")
