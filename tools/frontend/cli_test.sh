#!/bin/bash
set -euo pipefail

cli_directory="$(cd "$(dirname "$1")" && pwd)"
cli="${cli_directory}/$(basename "$1")"
source_fixture="$2"
invalid_fixture="$3"
golden_diagnostic="$4"
rust_generator="$5"
equivalence="$6"

first_dir="${TEST_TMPDIR}/first"
second_dir="${TEST_TMPDIR}/second"
rust_dir="${TEST_TMPDIR}/rust"
copied_source="${TEST_TMPDIR}/copied/renamed.circuitc"
mkdir -p "$(dirname "${copied_source}")"
cp "${source_fixture}" "${copied_source}"

"${cli}" compile "${source_fixture}" --output-dir "${first_dir}"
"${cli}" compile "${copied_source}" --diagnostic-format human --output-dir "${second_dir}"
"${rust_generator}" "${rust_dir}"
cmp "${first_dir}/voltage_divider.kicad_pcb" "${second_dir}/voltage_divider.kicad_pcb"
cmp "${first_dir}/voltage_divider.spice" "${second_dir}/voltage_divider.spice"
cmp "${first_dir}/voltage_divider.kicad_pcb" "${rust_dir}/voltage_divider.kicad_pcb"
cmp "${first_dir}/voltage_divider.spice" "${rust_dir}/voltage_divider.spice"
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

set +e
"${cli}" compile "${TEST_TMPDIR}/missing.circuitc" --output-dir "${TEST_TMPDIR}/io" \
  >"${TEST_TMPDIR}/io.stdout" 2>"${TEST_TMPDIR}/io.stderr"
io_status=$?
set -e
if [[ ${io_status} -ne 3 ]]; then
  echo "expected I/O exit 3; found ${io_status}" >&2
  exit 1
fi
