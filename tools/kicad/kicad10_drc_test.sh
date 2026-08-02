#!/bin/bash
set -euo pipefail

generator="$1"
normalizer="$2"
frontend="$3"
source_fixture="$4"
project_validator="$5"
physical_source_fixture="$6"

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
rust_dir="${TEST_TMPDIR}/rust"
"${frontend}" compile "${source_fixture}" --output-dir "${first_dir}"
"${frontend}" compile "${source_fixture}" --output-dir "${second_dir}"
"${generator}" "${rust_dir}"
cmp "${first_dir}/voltage_divider.kicad_pcb" "${second_dir}/voltage_divider.kicad_pcb"
for artifact in \
  voltage_divider.kicad_sch \
  voltage_divider.kicad_pcb \
  voltage_divider.kicad_pro \
  voltage_divider.spice \
  CircuitC.kicad_sym \
  CircuitC.pretty/R_0603_1608Metric.kicad_mod \
  sym-lib-table \
  fp-lib-table; do
  cmp "${first_dir}/${artifact}" "${second_dir}/${artifact}"
  cmp "${first_dir}/${artifact}" "${rust_dir}/${artifact}"
done
cmp \
  "${first_dir}/voltage_divider.kicad-map.json" \
  "${second_dir}/voltage_divider.kicad-map.json"

for directory in "${first_dir}" "${second_dir}" "${rust_dir}"; do
  python3 "${project_validator}" \
    --project "${directory}/voltage_divider.kicad_pro" \
    --expected-filename voltage_divider.kicad_pro \
    --normalized "${directory}/project.normalized.json"
done
cmp "${first_dir}/project.normalized.json" "${second_dir}/project.normalized.json"
cmp "${first_dir}/project.normalized.json" "${rust_dir}/project.normalized.json"

mkdir -p "${TEST_TMPDIR}/symbol-svg" "${TEST_TMPDIR}/footprint-svg"
KICAD_CONFIG_HOME="${TEST_TMPDIR}/library-config" "${kicad_cli}" sym export svg \
  --output "${TEST_TMPDIR}/symbol-svg" \
  --symbol R \
  "${first_dir}/CircuitC.kicad_sym"
KICAD_CONFIG_HOME="${TEST_TMPDIR}/library-config" "${kicad_cli}" fp export svg \
  --output "${TEST_TMPDIR}/footprint-svg" \
  --footprint R_0603_1608Metric \
  "${first_dir}/CircuitC.pretty"

for directory in "${first_dir}" "${second_dir}"; do
  config_directory="${directory}/isolated-kicad-config"
  mkdir -p "${config_directory}"
  KICAD_CONFIG_HOME="${config_directory}" "${kicad_cli}" sch erc \
    --format json \
    --severity-all \
    --output "${directory}/erc.raw.json" \
    "${directory}/voltage_divider.kicad_sch"
  python3 "${normalizer}" \
    --raw "${directory}/erc.raw.json" \
    --normalized "${directory}/erc.normalized.json" \
    --expected-major 10 \
    --allow-ignored-check single_global_label \
    --allow-ignored-check four_way_junction \
    --allow-ignored-check simulation_model_issue \
    --allow-ignored-check footprint_filter \
    --identity-map "${directory}/voltage_divider.kicad-map.json"

  KICAD_CONFIG_HOME="${config_directory}" "${kicad_cli}" pcb drc \
    --format json \
    --severity-all \
    --schematic-parity \
    --output "${directory}/drc.raw.json" \
    "${directory}/voltage_divider.kicad_pcb"
  python3 "${normalizer}" \
    --raw "${directory}/drc.raw.json" \
    --normalized "${directory}/drc.normalized.json" \
    --expected-major 10 \
    --allow-ignored-check missing_courtyard \
    --allow-ignored-check track_not_centered_on_via \
    --allow-ignored-check tuning_profile_track_geometries \
    --allow-ignored-check footprint_filters_mismatch \
    --allow-ignored-check footprint_type_mismatch \
    --identity-map "${directory}/voltage_divider.kicad-map.json"
done

cmp "${first_dir}/erc.normalized.json" "${second_dir}/erc.normalized.json"
cmp "${first_dir}/drc.normalized.json" "${second_dir}/drc.normalized.json"
sed -n '1,240p' "${first_dir}/erc.normalized.json"
sed -n '1,240p' "${first_dir}/drc.normalized.json"

# Exercise the accepted physical-only/no-connect source state against the same
# host authorities. Two independent compilations make this a determinism test,
# not merely a one-off parser probe.
physical_first_dir="${TEST_TMPDIR}/physical-first"
physical_second_dir="${TEST_TMPDIR}/physical-second"
for directory in "${physical_first_dir}" "${physical_second_dir}"; do
  "${frontend}" compile "${physical_source_fixture}" --output-dir "${directory}"
  python3 "${project_validator}" \
    --project "${directory}/physical_no_connect.kicad_pro" \
    --expected-filename physical_no_connect.kicad_pro \
    --normalized "${directory}/project.normalized.json"

  config_directory="${directory}/isolated-kicad-config"
  mkdir -p "${config_directory}"
  KICAD_CONFIG_HOME="${config_directory}" "${kicad_cli}" sch erc \
    --format json \
    --severity-all \
    --output "${directory}/erc.raw.json" \
    "${directory}/physical_no_connect.kicad_sch"
  python3 "${normalizer}" \
    --raw "${directory}/erc.raw.json" \
    --normalized "${directory}/erc.normalized.json" \
    --expected-major 10 \
    --allow-ignored-check single_global_label \
    --allow-ignored-check four_way_junction \
    --allow-ignored-check simulation_model_issue \
    --allow-ignored-check footprint_filter \
    --identity-map "${directory}/physical_no_connect.kicad-map.json"

  KICAD_CONFIG_HOME="${config_directory}" "${kicad_cli}" pcb drc \
    --format json \
    --severity-all \
    --schematic-parity \
    --output "${directory}/drc.raw.json" \
    "${directory}/physical_no_connect.kicad_pcb"
  python3 "${normalizer}" \
    --raw "${directory}/drc.raw.json" \
    --normalized "${directory}/drc.normalized.json" \
    --expected-major 10 \
    --allow-ignored-check missing_courtyard \
    --allow-ignored-check track_not_centered_on_via \
    --allow-ignored-check tuning_profile_track_geometries \
    --allow-ignored-check footprint_filters_mismatch \
    --allow-ignored-check footprint_type_mismatch \
    --identity-map "${directory}/physical_no_connect.kicad-map.json"
done

for artifact in \
  physical_no_connect.kicad_sch \
  physical_no_connect.kicad_pcb \
  physical_no_connect.kicad_pro \
  physical_no_connect.spice \
  physical_no_connect.kicad-map.json \
  CircuitC.kicad_sym \
  CircuitC.pretty/R_0603_1608Metric.kicad_mod \
  sym-lib-table \
  fp-lib-table \
  project.normalized.json \
  erc.normalized.json \
  drc.normalized.json; do
  cmp "${physical_first_dir}/${artifact}" "${physical_second_dir}/${artifact}"
done
sed -n '1,240p' "${physical_first_dir}/erc.normalized.json"
sed -n '1,240p' "${physical_first_dir}/drc.normalized.json"
