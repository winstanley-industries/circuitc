#!/bin/bash
set -euo pipefail

frontend="$1"
fabrication_runner="$2"
fabrication_gate="$3"
analysis_gate="$4"
analysis_runner="$5"
normalizer_source="$6"
host_runner_source="$7"
source_fixture_source="$8"
catalog_source="$9"

cp "${source_fixture_source}" "${TEST_TMPDIR}/fixture.circuitc"
source_fixture="${TEST_TMPDIR}/fixture.circuitc"
cp "${catalog_source}" "${TEST_TMPDIR}/catalog.json"
catalog="${TEST_TMPDIR}/catalog.json"
cp "${normalizer_source}" "${TEST_TMPDIR}/normalize_drc.py"
normalizer="${TEST_TMPDIR}/normalize_drc.py"
cp "${host_runner_source}" "${TEST_TMPDIR}/run_host_validation.py"
host_runner="${TEST_TMPDIR}/run_host_validation.py"

if [[ -n "${CIRCUITC_KICAD_CLI:-}" ]]; then
  kicad_cli="${CIRCUITC_KICAD_CLI}"
elif command -v kicad-cli >/dev/null 2>&1; then
  kicad_cli="$(command -v kicad-cli)"
elif [[ -x /Applications/KiCad/KiCad.app/Contents/MacOS/kicad-cli ]]; then
  kicad_cli="/Applications/KiCad/KiCad.app/Contents/MacOS/kicad-cli"
else
  echo "KiCad 10.0.5 board-analysis gate unavailable: set CIRCUITC_KICAD_CLI" >&2
  exit 1
fi

version="$("${kicad_cli}" --version)"
if [[ "${version}" != "10.0.5" ]]; then
  echo "KiCad board-analysis gate requires exact version 10.0.5; found ${version}" >&2
  exit 1
fi

first_project="${TEST_TMPDIR}/first-project"
second_project="${TEST_TMPDIR}/second-project"
"${frontend}" compile "${source_fixture}" --output-dir "${first_project}"
"${frontend}" compile "${source_fixture}" --output-dir "${second_project}"
diff -r "${first_project}" "${second_project}"

"${fabrication_gate}" prepare \
  "${source_fixture}" \
  "${catalog}" \
  production \
  "${first_project}/voltage_divider.kicad_pcb" >"${TEST_TMPDIR}/fabrication-request.json"

python3 "${fabrication_runner}" \
  --kicad-cli "${kicad_cli}" \
  --request "${TEST_TMPDIR}/fabrication-request.json" \
  --board "${first_project}/voltage_divider.kicad_pcb" \
  --output-dir "${TEST_TMPDIR}/fabrication-raw" \
  --work-dir "${TEST_TMPDIR}/fabrication-work"

"${fabrication_gate}" bind \
  "${source_fixture}" \
  "${catalog}" \
  production \
  "${first_project}/voltage_divider.kicad_pcb" \
  "${TEST_TMPDIR}/fabrication-raw" \
  "${kicad_cli}" >"${TEST_TMPDIR}/fabrication-manifest.json"

"${analysis_gate}" prepare \
  "${source_fixture}" \
  "${catalog}" \
  production \
  "${TEST_TMPDIR}/fabrication-raw" \
  "${kicad_cli}" >"${TEST_TMPDIR}/analysis-request.json"

for pass in first second; do
  project="${first_project}"
  if [[ "${pass}" == second ]]; then
    project="${second_project}"
  fi
  "${analysis_runner}" \
    --request "${TEST_TMPDIR}/analysis-request.json" \
    --project-dir "${project}" \
    --fabrication-manifest "${TEST_TMPDIR}/fabrication-manifest.json" \
    --kicad-cli "${kicad_cli}" \
    --normalizer "${normalizer}" \
    --host-runner "${host_runner}" \
    --work-dir "${TEST_TMPDIR}/analysis-work" \
    --output-root "${TEST_TMPDIR}/${pass}-analysis-raw"

  "${analysis_gate}" bind \
    "${source_fixture}" \
    "${catalog}" \
    production \
    "${TEST_TMPDIR}/fabrication-raw" \
    "${kicad_cli}" \
    "${normalizer}" \
    "${host_runner}" \
    "${TEST_TMPDIR}/${pass}-analysis-raw" >"${TEST_TMPDIR}/${pass}-analysis-report.json"
done

cmp "${TEST_TMPDIR}/first-analysis-raw/erc.normalized.json" \
  "${TEST_TMPDIR}/second-analysis-raw/erc.normalized.json"
cmp "${TEST_TMPDIR}/first-analysis-raw/drc.normalized.json" \
  "${TEST_TMPDIR}/second-analysis-raw/drc.normalized.json"
cmp "${TEST_TMPDIR}/first-analysis-report.json" \
  "${TEST_TMPDIR}/second-analysis-report.json"
python3 - "${TEST_TMPDIR}/first-analysis-report.json" <<'PY'
import json
import pathlib
import sys

report = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
assert report["execution_status"] == "completed"
assert report["all_pass"] is True
assert [outcome["capability"] for outcome in report["outcomes"]] == [
    "erc_clean",
    "drc_clean",
    "unconnected_clean",
    "schematic_parity_clean",
    "fabrication_inventory_complete",
]
assert [outcome["outcome"] for outcome in report["outcomes"]] == ["pass"] * 5
assert [outcome["evidence_role"] for outcome in report["outcomes"]] == [
    "erc",
    "drc",
    "drc",
    "drc",
    "fabrication_manifest",
]
PY
if find "${TEST_TMPDIR}/analysis-work" -name 'analysis-*' -print -quit | grep -q .; then
  echo "board-analysis transactions were not cleaned up" >&2
  exit 1
fi
sed -n '1,4p' "${TEST_TMPDIR}/first-analysis-report.json"
