#!/bin/bash
set -euo pipefail

normalizer="$1"
first_raw="$2"
second_raw="$3"
unexpected_raw="$4"
erc_raw="$5"
identity_map="$6"
first_normalized="${TEST_TMPDIR}/first.normalized.json"
second_normalized="${TEST_TMPDIR}/second.normalized.json"

expect_failure() {
  local label="$1"
  local expected="$2"
  shift 2
  if python3 "${normalizer}" "$@" \
    >"${TEST_TMPDIR}/${label}.stdout" \
    2>"${TEST_TMPDIR}/${label}.stderr"; then
    echo "normalizer accepted ${label}" >&2
    exit 1
  fi
  grep -F "${expected}" "${TEST_TMPDIR}/${label}.stderr"
}

python3 "${normalizer}" \
  --raw "${first_raw}" \
  --normalized "${first_normalized}" \
  --expected-major 10 \
  --allow-library-warning R1 \
  --allow-library-warning R2 \
  --allow-ignored-check missing_courtyard \
  --allow-ignored-check track_not_centered_on_via
python3 "${normalizer}" \
  --raw "${second_raw}" \
  --normalized "${second_normalized}" \
  --expected-major 10 \
  --allow-library-warning R1 \
  --allow-library-warning R2 \
  --allow-ignored-check missing_courtyard \
  --allow-ignored-check track_not_centered_on_via
cmp "${first_normalized}" "${second_normalized}"

expect_failure unexpected-drc board.routes.vout_bridge \
  --raw "${unexpected_raw}" \
  --normalized "${TEST_TMPDIR}/unexpected.normalized.json" \
  --expected-major 10 \
  --identity-map "${identity_map}"

python3 "${normalizer}" \
  --raw "${erc_raw}" \
  --normalized "${TEST_TMPDIR}/erc.normalized.json" \
  --expected-major 10 \
  --allow-ignored-check simulation_model_issue
grep -F '"report_kind": "erc"' "${TEST_TMPDIR}/erc.normalized.json"

expect_failure erc-version-mismatch 'does not match supported version' \
  --raw "${erc_raw}" \
  --normalized "${TEST_TMPDIR}/erc-mismatch.normalized.json" \
  --expected-major 9 \
  --allow-ignored-check simulation_model_issue

expect_failure erc-library-allowlist \
  'library-warning allowlists apply only to DRC reports' \
  --raw "${erc_raw}" \
  --normalized "${TEST_TMPDIR}/erc-allowlist.normalized.json" \
  --expected-major 10 \
  --identity-map "${identity_map}" \
  --allow-library-warning R1

python3 - "${unexpected_raw}" "${TEST_TMPDIR}" <<'PY'
import copy
import json
import pathlib
import sys

source = pathlib.Path(sys.argv[1])
target = pathlib.Path(sys.argv[2])
report = json.loads(source.read_text(encoding="utf-8"))
finding = report["violations"][0]
for category, filename in (
    ("unconnected_items", "drc-unconnected.json"),
    ("schematic_parity", "drc-parity.json"),
):
    variant = copy.deepcopy(report)
    variant["violations"] = []
    variant[category] = [finding]
    (target / filename).write_text(json.dumps(variant), encoding="utf-8")

missing_uuid = copy.deepcopy(report)
missing_uuid["violations"][0]["items"][0].pop("uuid")
(target / "drc-missing-uuid.json").write_text(
    json.dumps(missing_uuid), encoding="utf-8"
)

multiple = copy.deepcopy(report)
second = copy.deepcopy(finding)
second["description"] = "Second DRC failure"
second["items"][0]["description"] = "Second mapped DRC item"
second["type"] = "second_clearance"
multiple["violations"] = [finding, second]
(target / "drc-multiple.json").write_text(json.dumps(multiple), encoding="utf-8")
PY

expect_failure missing-finding-uuid \
  'requires a UUID when an identity map is supplied' \
  --raw "${TEST_TMPDIR}/drc-missing-uuid.json" \
  --normalized "${TEST_TMPDIR}/drc-missing-uuid.normalized.json" \
  --expected-major 10 \
  --identity-map "${identity_map}"

expect_failure unexpected-drc-multiple 'Track has insufficient clearance' \
  --raw "${TEST_TMPDIR}/drc-multiple.json" \
  --normalized "${TEST_TMPDIR}/drc-multiple.normalized.json" \
  --expected-major 10 \
  --identity-map "${identity_map}"
grep -F 'Second DRC failure' "${TEST_TMPDIR}/unexpected-drc-multiple.stderr"

expect_failure unexpected-unconnected board.routes.vout_bridge \
  --raw "${TEST_TMPDIR}/drc-unconnected.json" \
  --normalized "${TEST_TMPDIR}/drc-unconnected.normalized.json" \
  --expected-major 10 \
  --identity-map "${identity_map}"
grep -F 'unconnected_items' "${TEST_TMPDIR}/unexpected-unconnected.stderr"

expect_failure unexpected-parity board.routes.vout_bridge \
  --raw "${TEST_TMPDIR}/drc-parity.json" \
  --normalized "${TEST_TMPDIR}/drc-parity.normalized.json" \
  --expected-major 10 \
  --identity-map "${identity_map}"
grep -F 'schematic_parity' "${TEST_TMPDIR}/unexpected-parity.stderr"

python3 - "${erc_raw}" "${TEST_TMPDIR}" <<'PY'
import copy
import json
import pathlib
import sys

report = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
target = pathlib.Path(sys.argv[2])
uuid = "33333333-3333-8333-8333-333333333333"
multiple = copy.deepcopy(report)
multiple["sheets"][0]["violations"] = [
    {
        "description": "First ERC failure",
        "items": [{"description": "First mapped item", "uuid": uuid}],
        "severity": "error",
        "type": "first_failure",
    },
    {
        "description": "Second ERC failure",
        "items": [{"description": "Second mapped item", "uuid": uuid}],
        "severity": "warning",
        "type": "second_failure",
    },
]
(target / "erc-multiple.json").write_text(json.dumps(multiple), encoding="utf-8")

sheet_not_object = copy.deepcopy(report)
sheet_not_object["sheets"] = ["not-an-object"]
(target / "erc-sheet-not-object.json").write_text(
    json.dumps(sheet_not_object), encoding="utf-8"
)

missing_uuid_path = copy.deepcopy(report)
missing_uuid_path["sheets"][0].pop("uuid_path")
(target / "erc-missing-uuid-path.json").write_text(
    json.dumps(missing_uuid_path), encoding="utf-8"
)

violations_not_list = copy.deepcopy(report)
violations_not_list["sheets"][0]["violations"] = {}
(target / "erc-violations-not-list.json").write_text(
    json.dumps(violations_not_list), encoding="utf-8"
)
PY

expect_failure erc-sheet-not-object 'every KiCad ERC sheet must be an object' \
  --raw "${TEST_TMPDIR}/erc-sheet-not-object.json" \
  --normalized "${TEST_TMPDIR}/erc-sheet-not-object.normalized.json" \
  --expected-major 10 \
  --allow-ignored-check simulation_model_issue

expect_failure erc-missing-uuid-path 'requires path and uuid_path' \
  --raw "${TEST_TMPDIR}/erc-missing-uuid-path.json" \
  --normalized "${TEST_TMPDIR}/erc-missing-uuid-path.normalized.json" \
  --expected-major 10 \
  --allow-ignored-check simulation_model_issue

expect_failure erc-violations-not-list 'requires a violations list' \
  --raw "${TEST_TMPDIR}/erc-violations-not-list.json" \
  --normalized "${TEST_TMPDIR}/erc-violations-not-list.normalized.json" \
  --expected-major 10 \
  --allow-ignored-check simulation_model_issue

expect_failure unexpected-erc 'First ERC failure' \
  --raw "${TEST_TMPDIR}/erc-multiple.json" \
  --normalized "${TEST_TMPDIR}/erc-multiple.normalized.json" \
  --expected-major 10 \
  --allow-ignored-check simulation_model_issue \
  --identity-map "${identity_map}"
grep -F 'Second ERC failure' "${TEST_TMPDIR}/unexpected-erc.stderr"
grep -F 'board.routes.vout_bridge' "${TEST_TMPDIR}/unexpected-erc.stderr"

python3 - "${erc_raw}" "${TEST_TMPDIR}/wrong-severities.json" <<'PY'
import json
import pathlib
import sys

report = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
report["included_severities"] = ["error", "warning"]
pathlib.Path(sys.argv[2]).write_text(json.dumps(report), encoding="utf-8")
PY

expect_failure wrong-severities 'included severities do not match required policy' \
  --raw "${TEST_TMPDIR}/wrong-severities.json" \
  --normalized "${TEST_TMPDIR}/wrong-severities.normalized.json" \
  --expected-major 10 \
  --allow-ignored-check simulation_model_issue

expect_failure unexpected-ignored-check 'ignored checks do not match the allowlist' \
  --raw "${erc_raw}" \
  --normalized "${TEST_TMPDIR}/unexpected-ignored.normalized.json" \
  --expected-major 10

python3 - "${identity_map}" "${TEST_TMPDIR}" <<'PY'
import copy
import json
import pathlib
import sys

source = pathlib.Path(sys.argv[1])
target = pathlib.Path(sys.argv[2])
manifest = json.loads(source.read_text(encoding="utf-8"))

variants = {}
variant = copy.deepcopy(manifest)
variant["schema_version"] = 2
variants["identity-unsupported-schema.json"] = variant
variant = copy.deepcopy(manifest)
variant["schema_version"] = True
variants["identity-boolean-schema.json"] = variant
variant = copy.deepcopy(manifest)
variant.pop("identities")
variants["identity-missing-identities.json"] = variant
variant = copy.deepcopy(manifest)
variant.pop("source")
variants["identity-missing-source.json"] = variant
variant = copy.deepcopy(manifest)
duplicate = copy.deepcopy(variant["identities"][0])
duplicate["semantic_path"] += ".duplicate"
variant["identities"].append(duplicate)
variants["identity-duplicate-uuid.json"] = variant
variant = copy.deepcopy(manifest)
variant["identities"][0]["location"]["start"] = -1
variants["identity-invalid-location.json"] = variant
variant = copy.deepcopy(manifest)
duplicate = copy.deepcopy(variant["identities"][0])
duplicate["uuid"] = "44444444-4444-8444-8444-444444444444"
variant["identities"].append(duplicate)
variants["identity-duplicate-path.json"] = variant
variant = copy.deepcopy(manifest)
variant["source"] = "/absolute/voltage_divider.circuitc"
variants["identity-nonlogical-source.json"] = variant
variant = copy.deepcopy(manifest)
variant["source"] = "other.circuitc"
variants["identity-source-mismatch.json"] = variant
variant = copy.deepcopy(manifest)
variant["identities"][0]["uuid"] = "not-a-uuid"
variants["identity-invalid-uuid.json"] = variant
variant = copy.deepcopy(manifest)
variant["identities"][0]["semantic_path"] = "../outside"
variants["identity-invalid-semantic-path.json"] = variant
variant = copy.deepcopy(manifest)
variant["unexpected"] = True
variants["identity-extra-manifest-field.json"] = variant
variant = copy.deepcopy(manifest)
variant["identities"][0]["unexpected"] = True
variants["identity-extra-identity-field.json"] = variant
variant = copy.deepcopy(manifest)
variant["identities"][0]["location"]["unexpected"] = 1
variants["identity-extra-location-field.json"] = variant

for filename, contents in variants.items():
    (target / filename).write_text(json.dumps(contents), encoding="utf-8")
PY

for case in \
  'unsupported-schema:unsupported CircuitC KiCad identity map' \
  'boolean-schema:unsupported CircuitC KiCad identity map' \
  'missing-identities:requires an identities list' \
  'missing-source:requires a source string' \
  'duplicate-uuid:duplicate CircuitC KiCad UUID' \
  'invalid-location:identity location is out of range' \
  'duplicate-path:duplicate CircuitC KiCad semantic path' \
  'nonlogical-source:logical <design>.circuitc basename form' \
  'source-mismatch:does not match the KiCad report source' \
  'invalid-uuid:requires a canonical UUIDv8' \
  'invalid-semantic-path:requires a canonical semantic path' \
  'extra-manifest-field:must contain exactly schema_version, source, and identities' \
  'extra-identity-field:must contain exactly uuid, semantic_path, and location' \
  'extra-location-field:locations require exactly integer start, end, line, and column'; do
  label="${case%%:*}"
  expected="${case#*:}"
  expect_failure "identity-${label}" "${expected}" \
    --raw "${erc_raw}" \
    --normalized "${TEST_TMPDIR}/identity-${label}.normalized.json" \
    --expected-major 10 \
    --allow-ignored-check simulation_model_issue \
    --identity-map "${TEST_TMPDIR}/identity-${label}.json"
done

python3 - "${unexpected_raw}" "${TEST_TMPDIR}/unknown-finding-uuid.json" <<'PY'
import json
import pathlib
import sys

report = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
report["violations"][0]["items"][0]["uuid"] = "44444444-4444-8444-8444-444444444444"
pathlib.Path(sys.argv[2]).write_text(json.dumps(report), encoding="utf-8")
PY

expect_failure unknown-finding-uuid 'is absent from the identity map' \
  --raw "${TEST_TMPDIR}/unknown-finding-uuid.json" \
  --normalized "${TEST_TMPDIR}/unknown-finding-uuid.normalized.json" \
  --expected-major 10 \
  --identity-map "${identity_map}"
