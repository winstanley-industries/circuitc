#!/bin/bash
set -euo pipefail

frontend="$1"
host_runner="$2"
binder="$3"
source_fixture="$4"
catalog_snapshot="$5"

# Bazel data dependencies are exposed through a runfiles symlink forest. Stage
# ordinary files before exercising the production gate, which intentionally
# rejects symlinked authenticated inputs.
cp "${source_fixture}" "${TEST_TMPDIR}/fixture.circuitc"
source_fixture="${TEST_TMPDIR}/fixture.circuitc"
cp "${catalog_snapshot}" "${TEST_TMPDIR}/catalog.json"
catalog_snapshot="${TEST_TMPDIR}/catalog.json"

if [[ -n "${CIRCUITC_KICAD_CLI:-}" ]]; then
  kicad_cli="${CIRCUITC_KICAD_CLI}"
elif command -v kicad-cli >/dev/null 2>&1; then
  kicad_cli="$(command -v kicad-cli)"
elif [[ -x /Applications/KiCad/KiCad.app/Contents/MacOS/kicad-cli ]]; then
  kicad_cli="/Applications/KiCad/KiCad.app/Contents/MacOS/kicad-cli"
else
  echo "KiCad 10.0.5 manufacturing gate unavailable: set CIRCUITC_KICAD_CLI" >&2
  exit 1
fi

version="$("${kicad_cli}" --version)"
if [[ "${version}" != "10.0.5" ]]; then
  echo "KiCad manufacturing gate requires exact version 10.0.5; found ${version}" >&2
  exit 1
fi

first_compile="${TEST_TMPDIR}/first-compile"
second_compile="${TEST_TMPDIR}/second-compile"
"${frontend}" compile "${source_fixture}" --output-dir "${first_compile}"
"${frontend}" compile "${source_fixture}" --output-dir "${second_compile}"
cmp "${first_compile}/voltage_divider.kicad_pcb" "${second_compile}/voltage_divider.kicad_pcb"

"${binder}" prepare \
  "${source_fixture}" \
  "${catalog_snapshot}" \
  production \
  "${first_compile}/voltage_divider.kicad_pcb" >"${TEST_TMPDIR}/fabrication-request.json"

python3 "${host_runner}" \
  --kicad-cli "${kicad_cli}" \
  --request "${TEST_TMPDIR}/fabrication-request.json" \
  --board "${first_compile}/voltage_divider.kicad_pcb" \
  --output-dir "${TEST_TMPDIR}/first-raw" \
  --work-dir "${TEST_TMPDIR}/host-work"

# Ensure the two raw CreationDate values cannot accidentally share one second.
python3 -c 'import time; time.sleep(1.1)'

python3 "${host_runner}" \
  --kicad-cli "${kicad_cli}" \
  --request "${TEST_TMPDIR}/fabrication-request.json" \
  --board "${second_compile}/voltage_divider.kicad_pcb" \
  --output-dir "${TEST_TMPDIR}/second-raw" \
  --work-dir "${TEST_TMPDIR}/host-work"

if cmp -s \
  "${TEST_TMPDIR}/first-raw/gerber/voltage_divider-F_Cu.gbr" \
  "${TEST_TMPDIR}/second-raw/gerber/voltage_divider-F_Cu.gbr"; then
  echo "raw KiCad Gerbers unexpectedly retained identical host-clock fields" >&2
  exit 1
fi

"${binder}" bind \
  "${source_fixture}" \
  "${catalog_snapshot}" \
  production \
  "${first_compile}/voltage_divider.kicad_pcb" \
  "${TEST_TMPDIR}/first-raw" \
  "${kicad_cli}" >"${TEST_TMPDIR}/first-manifest.json"

"${binder}" bind \
  "${source_fixture}" \
  "${catalog_snapshot}" \
  production \
  "${second_compile}/voltage_divider.kicad_pcb" \
  "${TEST_TMPDIR}/second-raw" \
  "${kicad_cli}" >"${TEST_TMPDIR}/second-manifest.json"

cmp "${TEST_TMPDIR}/first-manifest.json" "${TEST_TMPDIR}/second-manifest.json"
sed -n '1,4p' "${TEST_TMPDIR}/first-manifest.json"
