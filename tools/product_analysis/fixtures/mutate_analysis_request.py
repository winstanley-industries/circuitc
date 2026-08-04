#!/usr/bin/env python3
import argparse
import hashlib
import json
import pathlib

DOMAIN = b"CIRCUITC-BOARD-ANALYSIS-IDENTITY-V1\0"

parser = argparse.ArgumentParser()
parser.add_argument("--input", required=True, type=pathlib.Path)
parser.add_argument("--output", required=True, type=pathlib.Path)
parser.add_argument("--mode", required=True, choices=("reorder-policy", "boolean-resource"))
args = parser.parse_args()

request = json.loads(args.input.read_text())
if args.mode == "reorder-policy":
    policy = request["policy"]
    request["policy"] = {
        "erc_ignored_checks": policy["erc_ignored_checks"],
        "included_severities": policy["included_severities"],
        "drc_ignored_checks": policy["drc_ignored_checks"],
        "drc_library_warning": policy["drc_library_warning"],
    }
else:
    request["resources"]["timeout_ms"] = True

preimage = {
    key: value
    for key, value in request.items()
    if key not in {"schema_name", "schema_version", "analysis_identity_sha256"}
}
request["analysis_identity_sha256"] = hashlib.sha256(
    DOMAIN + json.dumps(preimage, separators=(",", ":")).encode()
).hexdigest()
args.output.write_text(json.dumps(request, separators=(",", ":")) + "\n")
