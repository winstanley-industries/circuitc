#!/bin/bash
set -euo pipefail

binder="$1"
route_verifier="$2"
provenance="$3"
frontend="$4"
fixture="$5"
root="${TEST_TMPDIR}/route-acceptance"
compiled="${root}/compiled"
mkdir -p "${root}"
"${frontend}" compile "${fixture}" --output-dir "${compiled}"
route_dir="$(find "${compiled}/routing" -mindepth 1 -maxdepth 1 -type d)"
request="${route_dir}/request.json"
result="${route_dir}/result.json"
projection="${route_dir}/projection.json"
pcb="${compiled}/routed_voltage_divider.kicad_pcb"
schematic="${compiled}/routed_voltage_divider.kicad_sch"
drc="${root}/drc.normalized.json"
erc="${root}/erc.normalized.json"

python3 - "${drc}" "${erc}" "${pcb}" "${schematic}" <<'PY'
import hashlib
import json
import pathlib
import sys

drc = {
    "schema_version": 1,
    "report_kind": "drc",
    "host": {"name": "kicad", "major": 10, "version": "10.0.5"},
    "source": "routed_voltage_divider.kicad_pcb",
    "coordinate_units": "mm",
    "included_severities": ["error", "exclusion", "warning"],
    "ignored_checks": [
        {"key": "footprint_filters_mismatch", "description": "Footprint doesn't match symbol's footprint filters"},
        {"key": "footprint_type_mismatch", "description": "Footprint component type doesn't match footprint pads"},
        {"key": "missing_courtyard", "description": "Footprint has no courtyard defined"},
        {"key": "track_not_centered_on_via", "description": "Track endpoint not centered on via"},
        {"key": "tuning_profile_track_geometries", "description": "Tuning profile track geometries"},
    ],
    "schematic_parity": [],
    "unconnected_items": [],
    "violations": [],
    "source_sha256": hashlib.sha256(pathlib.Path(sys.argv[3]).read_bytes()).hexdigest(),
}
erc = {
    "schema_version": 1,
    "report_kind": "erc",
    "host": {"name": "kicad", "major": 10, "version": "10.0.5"},
    "source": "routed_voltage_divider.kicad_sch",
    "coordinate_units": "mm",
    "included_severities": ["error", "exclusion", "warning"],
    "ignored_checks": [
        {"key": "footprint_filter", "description": "Assigned footprint doesn't match footprint filters"},
        {"key": "four_way_junction", "description": "Four connection points are joined together"},
        {"key": "simulation_model_issue", "description": "SPICE model issue"},
        {"key": "single_global_label", "description": "Global label only appears once in the schematic"},
    ],
    "sheets": [{"path": "/", "uuid_path": "/", "violations": []}],
    "source_sha256": hashlib.sha256(pathlib.Path(sys.argv[4]).read_bytes()).hexdigest(),
}
for path, value in ((pathlib.Path(sys.argv[1]), drc), (pathlib.Path(sys.argv[2]), erc)):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")
PY

bind() {
  local output="$1"
  shift
  python3 "${binder}" \
    --request "${request}" \
    --result "${result}" \
    --projection "${projection}" \
    --pcb "${pcb}" \
    --schematic "${schematic}" \
    --drc "${drc}" \
    --erc "${erc}" \
    --provenance "${provenance}" \
    --route-verifier "${route_verifier}" \
    --output "${output}" \
    "$@"
}

bind "${root}/acceptance-first.json"
bind "${root}/acceptance-second.json"
cmp "${root}/acceptance-first.json" "${root}/acceptance-second.json"
grep -F '"apgar_exact_admission":true' "${root}/acceptance-first.json"
grep -F '"kicad_drc_clean":true' "${root}/acceptance-first.json"

expect_failure() {
  local label="$1"
  local expected="$2"
  shift 2
  if python3 "${binder}" "$@" --output "${root}/${label}.acceptance.json" \
    >"${root}/${label}.stdout" 2>"${root}/${label}.stderr"; then
    echo "route acceptance binder accepted ${label}" >&2
    exit 1
  fi
  grep -F "${expected}" "${root}/${label}.stderr"
}

python3 - "${result}" "${projection}" "${drc}" "${root}" <<'PY'
import copy
import hashlib
import json
import pathlib
import sys

result_path, projection_path, drc_path, root_path = map(pathlib.Path, sys.argv[1:])
root = json.loads(result_path.read_text(encoding="utf-8"))
projection = json.loads(projection_path.read_text(encoding="utf-8"))

stale = copy.deepcopy(root)
stale["request_sha256"] = "0" * 64
(root_path / "stale-result.json").write_text(
    json.dumps(stale, separators=(",", ":")) + "\n", encoding="utf-8"
)

unadmitted = copy.deepcopy(root)
unadmitted["outcome"]["candidates"][0]["constraints"][
    "exact_validation_status"
] = "failed"
unadmitted_bytes = (json.dumps(unadmitted, separators=(",", ":")) + "\n").encode()
(root_path / "unadmitted-result.json").write_bytes(unadmitted_bytes)
unadmitted_projection = copy.deepcopy(projection)
unadmitted_projection["result_sha256"] = hashlib.sha256(unadmitted_bytes).hexdigest()
(root_path / "unadmitted-projection.json").write_text(
    json.dumps(unadmitted_projection, separators=(",", ":")) + "\n", encoding="utf-8"
)

odd = copy.deepcopy(root)
odd["outcome"]["candidates"][0]["geometry"][0]["start"]["x"] += 1
odd_bytes = (json.dumps(odd, separators=(",", ":")) + "\n").encode()
(root_path / "odd-result.json").write_bytes(odd_bytes)
odd_projection = copy.deepcopy(projection)
odd_projection["result_sha256"] = hashlib.sha256(odd_bytes).hexdigest()
(root_path / "odd-projection.json").write_text(
    json.dumps(odd_projection, separators=(",", ":")) + "\n", encoding="utf-8"
)

dirty_drc = json.loads(drc_path.read_text(encoding="utf-8"))
dirty_drc["violations"] = [{"description": "mutant"}]
(root_path / "dirty-drc.json").write_text(
    json.dumps(dirty_drc, indent=2, sort_keys=True) + "\n", encoding="utf-8"
)

ignored_clearance = json.loads(drc_path.read_text(encoding="utf-8"))
ignored_clearance["ignored_checks"].append(
    {"key": "clearance", "description": "Clearance violation"}
)
(root_path / "ignored-clearance-drc.json").write_text(
    json.dumps(ignored_clearance, indent=2, sort_keys=True) + "\n", encoding="utf-8"
)

forged = copy.deepcopy(root)
forged["outcome"]["candidates"][0]["geometry_signature"] = "0" * 32
forged_bytes = (json.dumps(forged, separators=(",", ":")) + "\n").encode()
(root_path / "forged-result.json").write_bytes(forged_bytes)
forged_projection = copy.deepcopy(projection)
forged_projection["candidate_geometry_signature"] = "0" * 32
forged_projection["result_sha256"] = hashlib.sha256(forged_bytes).hexdigest()
(root_path / "forged-projection.json").write_text(
    json.dumps(forged_projection, separators=(",", ":")) + "\n", encoding="utf-8"
)
PY

common_args=(
  --request "${request}"
  --projection "${projection}"
  --pcb "${pcb}"
  --schematic "${schematic}"
  --drc "${drc}"
  --erc "${erc}"
  --provenance "${provenance}"
  --route-verifier "${route_verifier}"
)
expect_failure stale-result 'strict APGAR evidence verifier rejected input' \
  "${common_args[@]}" --result "${root}/stale-result.json"
expect_failure unadmitted-result 'strict APGAR evidence verifier rejected input' \
  --request "${request}" \
  --result "${root}/unadmitted-result.json" \
  --projection "${root}/unadmitted-projection.json" \
  --pcb "${pcb}" \
  --schematic "${schematic}" \
  --drc "${drc}" \
  --erc "${erc}" \
  --provenance "${provenance}" \
  --route-verifier "${route_verifier}"
expect_failure odd-coordinate 'strict APGAR evidence verifier rejected input' \
  --request "${request}" \
  --result "${root}/odd-result.json" \
  --projection "${root}/odd-projection.json" \
  --pcb "${pcb}" \
  --schematic "${schematic}" \
  --drc "${drc}" \
  --erc "${erc}" \
  --provenance "${provenance}" \
  --route-verifier "${route_verifier}"
expect_failure forged-signature 'strict APGAR evidence verifier rejected input' \
  --request "${request}" \
  --result "${root}/forged-result.json" \
  --projection "${root}/forged-projection.json" \
  --pcb "${pcb}" \
  --schematic "${schematic}" \
  --drc "${drc}" \
  --erc "${erc}" \
  --provenance "${provenance}" \
  --route-verifier "${route_verifier}"
expect_failure dirty-drc 'drc.violations must be empty' \
  --request "${request}" \
  --result "${result}" \
  --projection "${projection}" \
  --pcb "${pcb}" \
  --schematic "${schematic}" \
  --drc "${root}/dirty-drc.json" \
  --erc "${erc}" \
  --provenance "${provenance}" \
  --route-verifier "${route_verifier}"
expect_failure ignored-clearance 'drc.ignored_checks does not match' \
  --request "${request}" \
  --result "${result}" \
  --projection "${projection}" \
  --pcb "${pcb}" \
  --schematic "${schematic}" \
  --drc "${root}/ignored-clearance-drc.json" \
  --erc "${erc}" \
  --provenance "${provenance}" \
  --route-verifier "${route_verifier}"

mutant_pcb="${root}/mutant.kicad_pcb"
cp "${pcb}" "${mutant_pcb}"
printf '\n' >>"${mutant_pcb}"
expect_failure changed-pcb 'projection PCB digest' \
  --request "${request}" \
  --result "${result}" \
  --projection "${projection}" \
  --pcb "${mutant_pcb}" \
  --schematic "${schematic}" \
  --drc "${drc}" \
  --erc "${erc}" \
  --provenance "${provenance}" \
  --route-verifier "${route_verifier}"

coordinated_dir="${root}/coordinated"
mkdir -p "${coordinated_dir}"
coordinated_pcb="${coordinated_dir}/routed_voltage_divider.kicad_pcb"
coordinated_projection="${coordinated_dir}/projection.json"
sed 's/(start 24 10)/(start 23 10)/' "${pcb}" >"${coordinated_pcb}"
python3 - "${projection}" "${coordinated_pcb}" "${coordinated_projection}" <<'PY'
import hashlib
import json
import pathlib
import sys

source, pcb, target = map(pathlib.Path, sys.argv[1:])
projection = json.loads(source.read_text(encoding="utf-8"))
projection["kicad_pcb_sha256"] = hashlib.sha256(pcb.read_bytes()).hexdigest()
target.write_text(json.dumps(projection, separators=(",", ":")) + "\n", encoding="utf-8")
PY
expect_failure coordinated-pcb 'does not match authenticated APGAR geometry' \
  --request "${request}" \
  --result "${result}" \
  --projection "${coordinated_projection}" \
  --pcb "${coordinated_pcb}" \
  --schematic "${schematic}" \
  --drc "${drc}" \
  --erc "${erc}" \
  --provenance "${provenance}" \
  --route-verifier "${route_verifier}"

uuid_dir="${root}/coordinated-uuid"
mkdir -p "${uuid_dir}"
uuid_pcb="${uuid_dir}/routed_voltage_divider.kicad_pcb"
uuid_projection="${uuid_dir}/projection.json"
python3 - "${projection}" "${pcb}" "${uuid_projection}" "${uuid_pcb}" <<'PY'
import hashlib
import json
import pathlib
import sys

projection_source, pcb_source, projection_target, pcb_target = map(pathlib.Path, sys.argv[1:])
projection = json.loads(projection_source.read_text(encoding="utf-8"))
old_uuid = projection["segments"][0]["kicad_uuid"]
replacement = "0" if old_uuid[0] != "0" else "1"
new_uuid = replacement + old_uuid[1:]
pcb_data = pcb_source.read_bytes()
if pcb_data.count(old_uuid.encode()) != 1:
    raise SystemExit("expected exactly one projected UUID in PCB mutant source")
pcb_data = pcb_data.replace(old_uuid.encode(), new_uuid.encode())
pcb_target.write_bytes(pcb_data)
projection["segments"][0]["kicad_uuid"] = new_uuid
projection["kicad_pcb_sha256"] = hashlib.sha256(pcb_data).hexdigest()
projection_target.write_text(
    json.dumps(projection, separators=(",", ":")) + "\n", encoding="utf-8"
)
PY
expect_failure coordinated-uuid 'projection geometry disagrees with strict APGAR evidence' \
  --request "${request}" \
  --result "${result}" \
  --projection "${uuid_projection}" \
  --pcb "${uuid_pcb}" \
  --schematic "${schematic}" \
  --drc "${drc}" \
  --erc "${erc}" \
  --provenance "${provenance}" \
  --route-verifier "${route_verifier}"

stale_dir="${root}/stale"
mkdir -p "${stale_dir}"
stale_schematic="${stale_dir}/routed_voltage_divider.kicad_sch"
cp "${schematic}" "${stale_schematic}"
printf '\n' >>"${stale_schematic}"
expect_failure stale-erc 'does not bind the exact source artifact bytes' \
  --request "${request}" \
  --result "${result}" \
  --projection "${projection}" \
  --pcb "${pcb}" \
  --schematic "${stale_schematic}" \
  --drc "${drc}" \
  --erc "${erc}" \
  --provenance "${provenance}" \
  --route-verifier "${route_verifier}"

wrong_pcb="${root}/wrong-name.kicad_pcb"
cp "${pcb}" "${wrong_pcb}"
expect_failure wrong-pcb-name 'artifact basenames do not match' \
  --request "${request}" \
  --result "${result}" \
  --projection "${projection}" \
  --pcb "${wrong_pcb}" \
  --schematic "${schematic}" \
  --drc "${drc}" \
  --erc "${erc}" \
  --provenance "${provenance}" \
  --route-verifier "${route_verifier}"
