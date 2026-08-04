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

reordered_projection = {key: projection[key] for key in reversed(projection)}
(root_path / "reordered-projection.json").write_text(
    json.dumps(reordered_projection, separators=(",", ":")) + "\n", encoding="utf-8"
)

reordered_segment_projection = copy.deepcopy(projection)
segment = reordered_segment_projection["segments"][0]
reordered_segment_projection["segments"][0] = {
    key: segment[key] for key in reversed(segment)
}
(root_path / "reordered-segment-projection.json").write_text(
    json.dumps(reordered_segment_projection, separators=(",", ":")) + "\n",
    encoding="utf-8",
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
dangling_target="${root}/redirected-output.json"
ln -s "${dangling_target}" "${root}/dangling-output.acceptance.json"
expect_failure dangling-output 'acceptance output already exists' "${common_args[@]}" \
  --result "${result}"
test ! -e "${dangling_target}"

redirected_dir="${root}/redirected-directory"
mkdir -p "${redirected_dir}"
ln -s "${redirected_dir}" "${root}/symlink-output-parent"
if python3 "${binder}" \
  "${common_args[@]}" \
  --result "${result}" \
  --output "${root}/symlink-output-parent/redirected.json" \
  >"${root}/symlink-output-parent.stdout" \
  2>"${root}/symlink-output-parent.stderr"; then
  echo "route acceptance binder followed a symlinked output parent" >&2
  exit 1
fi
grep -F 'acceptance output path is not a secure directory chain' \
  "${root}/symlink-output-parent.stderr"
test ! -e "${redirected_dir}/redirected.json"

mutant_provenance="${root}/wrong-identity-provenance.txt"
sed 's/name=circuitc-apgar-route/name=mutant-route-tool/' \
  "${provenance}" >"${mutant_provenance}"
expect_failure wrong-provenance-identity \
  'APGAR provenance does not match the pinned CPU tool identity' \
  --request "${request}" \
  --result "${result}" \
  --projection "${projection}" \
  --pcb "${pcb}" \
  --schematic "${schematic}" \
  --drc "${drc}" \
  --erc "${erc}" \
  --provenance "${mutant_provenance}" \
  --route-verifier "${route_verifier}"
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
expect_failure reordered-projection 'projection does not use canonical field order' \
  --request "${request}" \
  --result "${result}" \
  --projection "${root}/reordered-projection.json" \
  --pcb "${pcb}" \
  --schematic "${schematic}" \
  --drc "${drc}" \
  --erc "${erc}" \
  --provenance "${provenance}" \
  --route-verifier "${route_verifier}"
expect_failure reordered-segment \
  'projection.segments[0] does not use canonical field order' \
  --request "${request}" \
  --result "${result}" \
  --projection "${root}/reordered-segment-projection.json" \
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

extra_dir="${root}/coordinated-extra-segment"
mkdir -p "${extra_dir}"
extra_pcb="${extra_dir}/routed_voltage_divider.kicad_pcb"
extra_projection="${extra_dir}/projection.json"
extra_drc="${extra_dir}/drc.normalized.json"
python3 - \
  "${pcb}" "${projection}" "${drc}" \
  "${extra_pcb}" "${extra_projection}" "${extra_drc}" <<'PY'
import hashlib
import json
import pathlib
import re
import sys

pcb_source, projection_source, drc_source, pcb_target, projection_target, drc_target = map(
    pathlib.Path, sys.argv[1:]
)
pcb_text = pcb_source.read_text(encoding="utf-8")
match = re.search(r"^  \(segment\n.*?^  \)$", pcb_text, re.MULTILINE | re.DOTALL)
if match is None:
    raise SystemExit("expected a KiCad segment in coordinated extra-segment source")
extra_uuid = "00000000-0000-8000-8000-000000000001"
if extra_uuid in pcb_text:
    raise SystemExit("coordinated extra-segment UUID unexpectedly collides")
extra = re.sub(
    r'    \(uuid "[0-9a-f-]+"\)',
    f'    (uuid "{extra_uuid}")',
    match.group(0),
    count=1,
)
closing = pcb_text.rfind("\n)")
if closing < 0:
    raise SystemExit("expected final KiCad PCB close")
mutant_pcb = (pcb_text[:closing] + "\n" + extra + pcb_text[closing:]).encode()
pcb_target.write_bytes(mutant_pcb)

projection = json.loads(projection_source.read_text(encoding="utf-8"))
projection["kicad_pcb_sha256"] = hashlib.sha256(mutant_pcb).hexdigest()
projection_target.write_text(
    json.dumps(projection, separators=(",", ":")) + "\n", encoding="utf-8"
)

drc = json.loads(drc_source.read_text(encoding="utf-8"))
drc["source_sha256"] = hashlib.sha256(mutant_pcb).hexdigest()
drc_target.write_text(
    json.dumps(drc, indent=2, sort_keys=True) + "\n", encoding="utf-8"
)
PY
expect_failure coordinated-extra-segment \
  'exact KiCad segment set does not match authenticated APGAR projection' \
  --request "${request}" \
  --result "${result}" \
  --projection "${extra_projection}" \
  --pcb "${extra_pcb}" \
  --schematic "${schematic}" \
  --drc "${extra_drc}" \
  --erc "${erc}" \
  --provenance "${provenance}" \
  --route-verifier "${route_verifier}"

alternate_dir="${root}/coordinated-alternate-segment"
mkdir -p "${alternate_dir}"
alternate_pcb="${alternate_dir}/routed_voltage_divider.kicad_pcb"
alternate_projection="${alternate_dir}/projection.json"
alternate_drc="${alternate_dir}/drc.normalized.json"
python3 - \
  "${extra_pcb}" "${extra_projection}" "${extra_drc}" \
  "${alternate_pcb}" "${alternate_projection}" "${alternate_drc}" <<'PY'
import hashlib
import json
import pathlib
import re
import sys

pcb_source, projection_source, drc_source, pcb_target, projection_target, drc_target = map(
    pathlib.Path, sys.argv[1:]
)
pcb_text = pcb_source.read_text(encoding="utf-8")
matches = list(re.finditer(r"^  \(segment\n.*?^  \)$", pcb_text, re.MULTILINE | re.DOTALL))
if len(matches) != 2:
    raise SystemExit("expected two canonical segments in alternate-whitespace mutant source")
extra = matches[1]
indented = "\n".join(f" {line}" for line in extra.group(0).splitlines())
mutant_pcb = (pcb_text[: extra.start()] + indented + pcb_text[extra.end() :]).encode()
pcb_target.write_bytes(mutant_pcb)

projection = json.loads(projection_source.read_text(encoding="utf-8"))
projection["kicad_pcb_sha256"] = hashlib.sha256(mutant_pcb).hexdigest()
projection_target.write_text(
    json.dumps(projection, separators=(",", ":")) + "\n", encoding="utf-8"
)

drc = json.loads(drc_source.read_text(encoding="utf-8"))
drc["source_sha256"] = hashlib.sha256(mutant_pcb).hexdigest()
drc_target.write_text(
    json.dumps(drc, indent=2, sort_keys=True) + "\n", encoding="utf-8"
)
PY
expect_failure coordinated-alternate-segment \
  'emitted KiCad PCB contains an unsupported segment encoding' \
  --request "${request}" \
  --result "${result}" \
  --projection "${alternate_projection}" \
  --pcb "${alternate_pcb}" \
  --schematic "${schematic}" \
  --drc "${alternate_drc}" \
  --erc "${erc}" \
  --provenance "${provenance}" \
  --route-verifier "${route_verifier}"

arc_dir="${root}/coordinated-extra-arc"
mkdir -p "${arc_dir}"
arc_pcb="${arc_dir}/routed_voltage_divider.kicad_pcb"
arc_projection="${arc_dir}/projection.json"
arc_drc="${arc_dir}/drc.normalized.json"
python3 - \
  "${pcb}" "${projection}" "${drc}" \
  "${arc_pcb}" "${arc_projection}" "${arc_drc}" <<'PY'
import hashlib
import json
import pathlib
import sys

pcb_source, projection_source, drc_source, pcb_target, projection_target, drc_target = map(
    pathlib.Path, sys.argv[1:]
)
pcb_text = pcb_source.read_text(encoding="utf-8")
marker = "  (embedded_fonts no)\n)\n"
if pcb_text.count(marker) != 1:
    raise SystemExit("expected one KiCad board close marker")
arc = """  (arc
    (start 16 10)
    (mid 20 14)
    (end 24 10)
    (width 0.25)
    (layer \"F.Cu\")
    (net \"VOUT\")
    (uuid \"00000000-0000-8000-8000-000000000003\")
  )
"""
mutant_pcb = pcb_text.replace(marker, arc + marker, 1).encode()
pcb_target.write_bytes(mutant_pcb)

projection = json.loads(projection_source.read_text(encoding="utf-8"))
projection["kicad_pcb_sha256"] = hashlib.sha256(mutant_pcb).hexdigest()
projection_target.write_text(
    json.dumps(projection, separators=(",", ":")) + "\n", encoding="utf-8"
)

drc = json.loads(drc_source.read_text(encoding="utf-8"))
drc["source_sha256"] = hashlib.sha256(mutant_pcb).hexdigest()
drc_target.write_text(
    json.dumps(drc, indent=2, sort_keys=True) + "\n", encoding="utf-8"
)
PY
expect_failure coordinated-extra-arc 'emitted KiCad PCB contains unsupported routed copper' \
  --request "${request}" \
  --result "${result}" \
  --projection "${arc_projection}" \
  --pcb "${arc_pcb}" \
  --schematic "${schematic}" \
  --drc "${arc_drc}" \
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
