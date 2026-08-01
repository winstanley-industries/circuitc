#!/bin/bash
set -euo pipefail

generator="$1"
normalizer="$2"

if [[ -n "${CIRCUITC_KICAD_CLI:-}" ]]; then
  kicad_cli="${CIRCUITC_KICAD_CLI}"
elif command -v kicad-cli >/dev/null 2>&1; then
  kicad_cli="$(command -v kicad-cli)"
elif [[ -x /Applications/KiCad/KiCad.app/Contents/MacOS/kicad-cli ]]; then
  kicad_cli="/Applications/KiCad/KiCad.app/Contents/MacOS/kicad-cli"
else
  echo "KiCad 10 host gate unavailable: set CIRCUITC_KICAD_CLI" >&2
  exit 1
fi

version="$("${kicad_cli}" --version)"
if [[ "${version}" != 10.* ]]; then
  echo "KiCad 10 host gate requires major version 10; found ${version}" >&2
  exit 1
fi

first_dir="${TEST_TMPDIR}/first"
second_dir="${TEST_TMPDIR}/second"
"${generator}" "${first_dir}"
"${generator}" "${second_dir}"
cmp "${first_dir}/voltage_divider.kicad_pcb" "${second_dir}/voltage_divider.kicad_pcb"
cmp "${first_dir}/voltage_divider.spice" "${second_dir}/voltage_divider.spice"

for directory in "${first_dir}" "${second_dir}"; do
  "${kicad_cli}" pcb drc \
    --format json \
    --severity-all \
    --output "${directory}/drc.raw.json" \
    "${directory}/voltage_divider.kicad_pcb"
  python3 "${normalizer}" \
    --raw "${directory}/drc.raw.json" \
    --normalized "${directory}/drc.normalized.json" \
    --expected-major 10 \
    --allow-library-warning R1 \
    --allow-library-warning R2
done

cmp "${first_dir}/drc.normalized.json" "${second_dir}/drc.normalized.json"
sed -n '1,240p' "${first_dir}/drc.normalized.json"
