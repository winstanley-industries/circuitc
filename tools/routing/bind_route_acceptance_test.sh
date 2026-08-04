#!/bin/bash
set -euo pipefail

binder="$1"
frontend="$2"
fixture="$3"
root="${TEST_TMPDIR}/route-acceptance"
compiled="${root}/compiled"
mkdir -p "${root}"
"${frontend}" compile "${fixture}" --output-dir "${compiled}"
route_dir="$(find "${compiled}/routing" -mindepth 1 -maxdepth 1 -type d)"
request="${route_dir}/request.json"
result="${route_dir}/result.json"
projection="${route_dir}/projection.json"
pcb="${compiled}/routed_voltage_divider.kicad_pcb"
drc="${root}/drc.normalized.json"
erc="${root}/erc.normalized.json"

python3 - "${drc}" "${erc}" <<'PY'
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
    "ignored_checks": [],
    "schematic_parity": [],
    "unconnected_items": [],
    "violations": [],
}
erc = {
    "schema_version": 1,
    "report_kind": "erc",
    "host": {"name": "kicad", "major": 10, "version": "10.0.5"},
    "source": "routed_voltage_divider.kicad_sch",
    "coordinate_units": "mm",
    "included_severities": ["error", "exclusion", "warning"],
    "ignored_checks": [],
    "sheets": [{"path": "/", "uuid_path": "/", "violations": []}],
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
    --drc "${drc}" \
    --erc "${erc}" \
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
PY

common_args=(
  --request "${request}"
  --projection "${projection}"
  --pcb "${pcb}"
  --drc "${drc}"
  --erc "${erc}"
)
expect_failure stale-result 'request digest chain' \
  "${common_args[@]}" --result "${root}/stale-result.json"
expect_failure unadmitted-result 'lacks exact APGAR admission' \
  --request "${request}" \
  --result "${root}/unadmitted-result.json" \
  --projection "${root}/unadmitted-projection.json" \
  --pcb "${pcb}" \
  --drc "${drc}" \
  --erc "${erc}"
expect_failure odd-coordinate 'not losslessly representable' \
  --request "${request}" \
  --result "${root}/odd-result.json" \
  --projection "${root}/odd-projection.json" \
  --pcb "${pcb}" \
  --drc "${drc}" \
  --erc "${erc}"
expect_failure dirty-drc 'drc.violations must be empty' \
  --request "${request}" \
  --result "${result}" \
  --projection "${projection}" \
  --pcb "${pcb}" \
  --drc "${root}/dirty-drc.json" \
  --erc "${erc}"

mutant_pcb="${root}/mutant.kicad_pcb"
cp "${pcb}" "${mutant_pcb}"
printf '\n' >>"${mutant_pcb}"
expect_failure changed-pcb 'projection PCB digest' \
  --request "${request}" \
  --result "${result}" \
  --projection "${projection}" \
  --pcb "${mutant_pcb}" \
  --drc "${drc}" \
  --erc "${erc}"
