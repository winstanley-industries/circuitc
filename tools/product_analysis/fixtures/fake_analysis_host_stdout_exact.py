#!/usr/bin/env python3
import argparse
import hashlib
import json
import sys

sys.stdout.buffer.write(b"x" * (1024 * 1024))
sys.stdout.buffer.flush()

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

with open(args.source_artifact, "rb") as source:
    source_bytes = source.read()
report = {
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
    report["sheets"] = [
        {
            "path": "/",
            "uuid_path": "/00000000-0000-8000-8000-000000000000",
            "violations": [],
        }
    ]
else:
    report["schematic_parity"] = []
    report["unconnected_items"] = []
    report["violations"] = []

with open(args.raw_output, "w", encoding="utf-8", newline="\n") as output:
    output.write("{}\n")
with open(args.normalized_output, "w", encoding="utf-8", newline="\n") as output:
    json.dump(report, output, indent=2, sort_keys=True)
    output.write("\n")
