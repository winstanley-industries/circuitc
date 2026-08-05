#!/usr/bin/env python3
import argparse
import hashlib
import json
import pathlib
import subprocess
import sys

parser = argparse.ArgumentParser()
parser.add_argument("--kicad-cli")
parser.add_argument("--normalizer")
parser.add_argument("--kind", choices=("erc", "drc"), required=True)
parser.add_argument("--source-artifact", required=True)
parser.add_argument("--identity-map")
parser.add_argument("--project-artifact", action="append", default=[])
parser.add_argument("--raw-output", required=True)
parser.add_argument("--normalized-output", required=True)
parser.add_argument("--work-dir")
parser.add_argument("--expected-major")
parser.add_argument("--allow-ignored-check", action="append", default=[])
parser.add_argument("--retain-findings", action="store_true")
args = parser.parse_args()

subprocess.run(
    [sys.executable, "-I", args.normalizer, "--probe"],
    check=True,
    env={"LC_ALL": "C", "PATH": "/usr/bin:/bin"},
)

if (pathlib.Path(args.source_artifact).parent / "voltage_divider.kicad_dru").exists():
    raise SystemExit("ambient unbound project file reached the host")

source_bytes = open(args.source_artifact, "rb").read()
common = {
    "coordinate_units": "mm",
    "host": {"major": 10, "name": "kicad", "version": "10.0.5"},
    "ignored_checks": [
        {"description": f"policy {key}", "key": key} for key in sorted(args.allow_ignored_check)
    ],
    "included_severities": ["error", "exclusion", "warning"],
    "report_kind": args.kind,
    "schema_version": 1,
    "source": args.source_artifact.rsplit("/", 1)[-1],
    "source_sha256": hashlib.sha256(source_bytes).hexdigest(),
}
if args.kind == "erc":
    common["sheets"] = [
        {
            "path": "/",
            "uuid_path": "/00000000-0000-8000-8000-000000000000",
            "violations": [],
        }
    ]
else:
    common["schematic_parity"] = []
    common["unconnected_items"] = []
    common["violations"] = []

with open(args.raw_output, "w", encoding="utf-8", newline="\n") as output:
    output.write("{}\n")
with open(args.normalized_output, "w", encoding="utf-8", newline="\n") as output:
    json.dump(common, output, indent=2, sort_keys=True)
    output.write("\n")
