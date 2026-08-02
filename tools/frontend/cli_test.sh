#!/bin/bash
set -euo pipefail

cli_directory="$(cd "$(dirname "$1")" && pwd)"
cli="${cli_directory}/$(basename "$1")"
source_fixture="$2"
invalid_fixture="$3"
golden_diagnostic="$4"
rust_generator="$5"
equivalence="$6"
project_validator="$7"
normalizer="$8"
clean_erc_fixture="$9"

first_dir="${TEST_TMPDIR}/first"
second_dir="${TEST_TMPDIR}/second"
rust_dir="${TEST_TMPDIR}/rust"
copied_source="${TEST_TMPDIR}/copied/renamed.circuitc"
mkdir -p "$(dirname "${copied_source}")"
cp "${source_fixture}" "${copied_source}"

"${cli}" compile "${source_fixture}" --output-dir "${first_dir}"
"${cli}" compile "${copied_source}" --diagnostic-format human --output-dir "${second_dir}"
"${rust_generator}" "${rust_dir}"
artifacts=(
  voltage_divider.kicad_sch
  voltage_divider.kicad_pcb
  voltage_divider.kicad_pro
  CircuitC.kicad_sym
  CircuitC.pretty/R_0603_1608Metric.kicad_mod
  sym-lib-table
  fp-lib-table
  voltage_divider.spice
)
for artifact in "${artifacts[@]}"; do
  cmp "${first_dir}/${artifact}" "${second_dir}/${artifact}"
  cmp "${first_dir}/${artifact}" "${rust_dir}/${artifact}"
done
cmp \
  "${first_dir}/voltage_divider.kicad-map.json" \
  "${second_dir}/voltage_divider.kicad-map.json"
python3 "${normalizer}" \
  --raw "${clean_erc_fixture}" \
  --normalized "${TEST_TMPDIR}/identity-map.normalized.json" \
  --expected-major 10 \
  --allow-ignored-check simulation_model_issue \
  --identity-map "${first_dir}/voltage_divider.kicad-map.json"
grep -F '"report_kind": "erc"' "${TEST_TMPDIR}/identity-map.normalized.json"
"${project_validator}" \
  --project "${first_dir}/voltage_divider.kicad_pro" \
  --expected-filename voltage_divider.kicad_pro \
  --normalized "${TEST_TMPDIR}/voltage_divider.project.normalized.json"
grep -F "\${KIPRJMOD}/CircuitC.kicad_sym" "${first_dir}/sym-lib-table"
grep -F "\${KIPRJMOD}/CircuitC.pretty" "${first_dir}/fp-lib-table"
"${equivalence}" "${source_fixture}"

set +e
"${cli}" compile "${source_fixture}" --output-dir "${TEST_TMPDIR}/unused" --watch \
  >"${TEST_TMPDIR}/bad-args.stdout" 2>"${TEST_TMPDIR}/bad-args.stderr"
argument_status=$?
set -e
if [[ ${argument_status} -ne 2 ]]; then
  echo "expected unsupported-option exit 2; found ${argument_status}" >&2
  exit 1
fi
# The backticks are literal diagnostic punctuation, not command substitution.
# shellcheck disable=SC2016
grep -F 'unsupported option `--watch`' "${TEST_TMPDIR}/bad-args.stderr"

invalid_dir="${TEST_TMPDIR}/invalid-case"
mkdir -p "${invalid_dir}/output"
cp "${invalid_fixture}" "${invalid_dir}/invalid.circuitc"
printf 'existing board\n' >"${invalid_dir}/output/invalid.kicad_pcb"
printf 'existing spice\n' >"${invalid_dir}/output/invalid.spice"
printf 'existing board\n' >"${invalid_dir}/expected.kicad_pcb"
printf 'existing spice\n' >"${invalid_dir}/expected.spice"
set +e
(
  cd "${invalid_dir}"
  "${cli}" compile invalid.circuitc --output-dir output --diagnostic-format=json \
    >diagnostic.stdout 2>diagnostic.json
)
source_status=$?
set -e
if [[ ${source_status} -ne 1 ]]; then
  echo "expected source-error exit 1; found ${source_status}" >&2
  exit 1
fi
cmp "${invalid_dir}/diagnostic.json" "${golden_diagnostic}"
cmp "${invalid_dir}/output/invalid.kicad_pcb" "${invalid_dir}/expected.kicad_pcb"
cmp "${invalid_dir}/output/invalid.spice" "${invalid_dir}/expected.spice"

intent_dir="${TEST_TMPDIR}/declared-intent-case"
mkdir -p "${intent_dir}/output"
sed '$d' "${source_fixture}" >"${intent_dir}/intent.circuitc"
printf '  analysis dc_operating_point divider.simulation.dc;\n' \
  >>"${intent_dir}/intent.circuitc"
tail -n 1 "${source_fixture}" >>"${intent_dir}/intent.circuitc"
printf 'existing board\n' >"${intent_dir}/output/voltage_divider.kicad_pcb"
printf 'existing spice\n' >"${intent_dir}/output/voltage_divider.spice"
"${cli}" compile "${intent_dir}/intent.circuitc" \
  --output-dir "${intent_dir}/output" --diagnostic-format=json \
  >"${intent_dir}/diagnostic.stdout" 2>"${intent_dir}/diagnostic.json"
if [[ -s "${intent_dir}/diagnostic.json" ]]; then
  echo "checked simulation intent unexpectedly emitted diagnostics" >&2
  cat "${intent_dir}/diagnostic.json" >&2
  exit 1
fi
cmp "${intent_dir}/output/voltage_divider.kicad_pcb" \
  "${first_dir}/voltage_divider.kicad_pcb"
cmp "${intent_dir}/output/voltage_divider.spice" \
  "${first_dir}/voltage_divider.spice"
intent_chains=("${intent_dir}/output"/simulation/*)
if [[ ${#intent_chains[@]} -ne 1 || ! -d "${intent_chains[0]}" ]]; then
  echo "checked simulation intent did not publish exactly one analysis chain" >&2
  find "${intent_dir}/output" -print >&2
  exit 1
fi
for artifact in analysis.spice request.json spice-map.json result.json report.json; do
  test -f "${intent_chains[0]}/${artifact}"
done
if [[ $(find "${intent_chains[0]}" -type f | wc -l) -ne 5 ]]; then
  echo "checked simulation chain contains an unexpected artifact set" >&2
  find "${intent_chains[0]}" -print >&2
  exit 1
fi
grep -F '"status": "completed"' "${intent_chains[0]}/result.json"
grep -F '"pass": 0' "${intent_chains[0]}/report.json"
grep -F '"fail": 0' "${intent_chains[0]}/report.json"
grep -F '"unsupported": 0' "${intent_chains[0]}/report.json"
grep -F '"unevaluated": 0' "${intent_chains[0]}/report.json"

atomic_dir="${TEST_TMPDIR}/atomic-output"
mkdir -p "${atomic_dir}/voltage_divider.spice"
printf 'existing board\n' >"${atomic_dir}/voltage_divider.kicad_pcb"
printf 'existing board\n' >"${TEST_TMPDIR}/expected-existing-board"
set +e
"${cli}" compile "${source_fixture}" --output-dir "${atomic_dir}" \
  >"${TEST_TMPDIR}/atomic.stdout" 2>"${TEST_TMPDIR}/atomic.stderr"
atomic_status=$?
set -e
if [[ ${atomic_status} -ne 3 ]]; then
  echo "expected atomic-output I/O exit 3; found ${atomic_status}" >&2
  exit 1
fi
cmp "${atomic_dir}/voltage_divider.kicad_pcb" "${TEST_TMPDIR}/expected-existing-board"
test -d "${atomic_dir}/voltage_divider.spice"
test ! -e "${atomic_dir}/CircuitC.pretty"

symlink_atomic_dir="${TEST_TMPDIR}/symlink-atomic-output"
external_footprint_dir="${TEST_TMPDIR}/external-footprints"
external_footprint="${external_footprint_dir}/R_0603_1608Metric.kicad_mod"
mkdir -p "${symlink_atomic_dir}" "${external_footprint_dir}"
ln -s "${external_footprint_dir}" "${symlink_atomic_dir}/CircuitC.pretty"
printf 'external footprint sentinel\n' >"${external_footprint}"
printf 'external footprint sentinel\n' >"${TEST_TMPDIR}/expected-external-footprint"
printf 'existing board sentinel\n' >"${symlink_atomic_dir}/voltage_divider.kicad_pcb"
printf 'existing board sentinel\n' >"${TEST_TMPDIR}/expected-symlink-board"
set +e
"${cli}" compile "${source_fixture}" --output-dir "${symlink_atomic_dir}" \
  >"${TEST_TMPDIR}/symlink-atomic.stdout" 2>"${TEST_TMPDIR}/symlink-atomic.stderr"
symlink_atomic_status=$?
set -e
if [[ ${symlink_atomic_status} -ne 3 ]]; then
  echo "expected symlinked-output I/O exit 3; found ${symlink_atomic_status}" >&2
  exit 1
fi
cmp "${external_footprint}" "${TEST_TMPDIR}/expected-external-footprint"
cmp \
  "${symlink_atomic_dir}/voltage_divider.kicad_pcb" \
  "${TEST_TMPDIR}/expected-symlink-board"
test -L "${symlink_atomic_dir}/CircuitC.pretty"

symlink_ancestor_real="${TEST_TMPDIR}/symlink-ancestor-real"
symlink_ancestor_link="${TEST_TMPDIR}/symlink-ancestor-link"
mkdir -p "${symlink_ancestor_real}"
ln -s "${symlink_ancestor_real}" "${symlink_ancestor_link}"
set +e
"${cli}" compile "${source_fixture}" --output-dir "${symlink_ancestor_link}/out" \
  >"${TEST_TMPDIR}/symlink-ancestor.stdout" \
  2>"${TEST_TMPDIR}/symlink-ancestor.stderr"
symlink_ancestor_status=$?
set -e
if [[ ${symlink_ancestor_status} -ne 3 ]]; then
  echo "expected symlink-ancestor I/O exit 3; found ${symlink_ancestor_status}" >&2
  exit 1
fi
grep -F 'CC-CLI-IO-002: failed to write output directory' \
  "${TEST_TMPDIR}/symlink-ancestor.stderr"
grep -F 'must be a non-symlinked directory' \
  "${TEST_TMPDIR}/symlink-ancestor.stderr"
test ! -e "${symlink_ancestor_real}/out"

set +e
"${cli}" compile "${TEST_TMPDIR}/missing.circuitc" --output-dir "${TEST_TMPDIR}/io" \
  >"${TEST_TMPDIR}/io.stdout" 2>"${TEST_TMPDIR}/io.stderr"
io_status=$?
set -e
if [[ ${io_status} -ne 3 ]]; then
  echo "expected I/O exit 3; found ${io_status}" >&2
  exit 1
fi
