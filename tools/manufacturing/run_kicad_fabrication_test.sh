#!/bin/bash
set -euo pipefail

runner="$1"
mutating_host="$2"
wrong_version_host="$3"
stdout_exact_host="$4"
stdout_over_host="$5"
frontend="$6"
binder="$7"
source_fixture="$8"
catalog_snapshot="$9"

# Bazel data dependencies are exposed through a runfiles symlink forest. Stage
# ordinary files before exercising the production gate, which intentionally
# rejects symlinked authenticated inputs.
cp "${source_fixture}" "${TEST_TMPDIR}/fixture.circuitc"
source_fixture="${TEST_TMPDIR}/fixture.circuitc"
cp "${catalog_snapshot}" "${TEST_TMPDIR}/catalog.json"
catalog_snapshot="${TEST_TMPDIR}/catalog.json"

cp "${mutating_host}" "${TEST_TMPDIR}/mutating-host"
chmod +x "${TEST_TMPDIR}/mutating-host"
mutating_host="${TEST_TMPDIR}/mutating-host"
cp "${wrong_version_host}" "${TEST_TMPDIR}/wrong-version-host"
chmod +x "${TEST_TMPDIR}/wrong-version-host"
wrong_version_host="${TEST_TMPDIR}/wrong-version-host"
cp "${stdout_exact_host}" "${TEST_TMPDIR}/stdout-exact-host"
chmod +x "${TEST_TMPDIR}/stdout-exact-host"
stdout_exact_host="${TEST_TMPDIR}/stdout-exact-host"
cp "${stdout_over_host}" "${TEST_TMPDIR}/stdout-over-host"
chmod +x "${TEST_TMPDIR}/stdout-over-host"
stdout_over_host="${TEST_TMPDIR}/stdout-over-host"
"${frontend}" compile "${source_fixture}" --output-dir "${TEST_TMPDIR}/compiled"
board_fixture="${TEST_TMPDIR}/compiled/voltage_divider.kicad_pcb"
"${binder}" prepare \
  "${source_fixture}" \
  "${catalog_snapshot}" \
  production \
  "${board_fixture}" >"${TEST_TMPDIR}/request.json"

expect_failure() {
  expected="$1"
  shift
  stderr_file="${TEST_TMPDIR}/stderr-$RANDOM"
  if "$@" >"${TEST_TMPDIR}/stdout-$RANDOM" 2>"${stderr_file}"; then
    echo "expected fabrication runner failure containing: ${expected}" >&2
    exit 1
  fi
  grep -F "${expected}" "${stderr_file}"
}

expect_failure \
  "fabrication host must be exactly KiCad 10.0.5" \
  python3 "${runner}" \
  --kicad-cli "${wrong_version_host}" \
  --request "${TEST_TMPDIR}/request.json" \
  --board "${board_fixture}" \
  --output-dir "${TEST_TMPDIR}/wrong-version-output" \
  --work-dir "${TEST_TMPDIR}/wrong-version-work"

python3 -c 'import json,pathlib,sys; source=pathlib.Path(sys.argv[1]); target=pathlib.Path(sys.argv[2]); value=json.loads(source.read_bytes()); board=value["kicad_pcb"]; value["kicad_pcb"]={"sha256":board["sha256"],"path":board["path"]}; target.write_text(json.dumps(value,ensure_ascii=False,separators=(",",":"))+"\n",encoding="utf-8")' \
  "${TEST_TMPDIR}/request.json" \
  "${TEST_TMPDIR}/reordered-request.json"
expect_failure \
  "fabrication request does not match the board or fixed KiCad profile" \
  python3 "${runner}" \
  --kicad-cli "${mutating_host}" \
  --request "${TEST_TMPDIR}/reordered-request.json" \
  --board "${board_fixture}" \
  --output-dir "${TEST_TMPDIR}/reordered-request-output" \
  --work-dir "${TEST_TMPDIR}/reordered-request-work"

expect_failure \
  "authenticated input changed during host export" \
  python3 "${runner}" \
  --kicad-cli "${mutating_host}" \
  --request "${TEST_TMPDIR}/request.json" \
  --board "${board_fixture}" \
  --output-dir "${TEST_TMPDIR}/mutation-output" \
  --work-dir "${TEST_TMPDIR}/mutation-work"

expect_failure \
  "KiCad Gerber export failed with exit 7" \
  python3 "${runner}" \
  --kicad-cli "${stdout_exact_host}" \
  --request "${TEST_TMPDIR}/request.json" \
  --board "${board_fixture}" \
  --output-dir "${TEST_TMPDIR}/stdout-exact-output" \
  --work-dir "${TEST_TMPDIR}/stdout-exact-work"

expect_failure \
  "KiCad Gerber export exceeded the bounded stdout/stderr budget" \
  python3 "${runner}" \
  --kicad-cli "${stdout_over_host}" \
  --request "${TEST_TMPDIR}/request.json" \
  --board "${board_fixture}" \
  --output-dir "${TEST_TMPDIR}/stdout-over-output" \
  --work-dir "${TEST_TMPDIR}/stdout-over-work"

ln -s "${board_fixture}" "${TEST_TMPDIR}/board-link.kicad_pcb"
expect_failure \
  "input is not a bounded regular file" \
  python3 "${runner}" \
  --kicad-cli "${mutating_host}" \
  --request "${TEST_TMPDIR}/request.json" \
  --board "${TEST_TMPDIR}/board-link.kicad_pcb" \
  --output-dir "${TEST_TMPDIR}/symlink-output" \
  --work-dir "${TEST_TMPDIR}/symlink-work"

ln -s "${mutating_host}" "${TEST_TMPDIR}/kicad-link"
expect_failure \
  "input is not a bounded regular file" \
  python3 "${runner}" \
  --kicad-cli "${TEST_TMPDIR}/kicad-link" \
  --request "${TEST_TMPDIR}/request.json" \
  --board "${board_fixture}" \
  --output-dir "${TEST_TMPDIR}/executable-symlink-output" \
  --work-dir "${TEST_TMPDIR}/executable-symlink-work"

create_raw_inventory() {
  root="$1"
  mkdir -p "${root}/gerber" "${root}/drill" "${root}/position" "${root}/receipt"
  for name in \
    voltage_divider-F_Cu.gbr \
    voltage_divider-F_Mask.gbr \
    voltage_divider-B_Cu.gbr \
    voltage_divider-B_Mask.gbr \
    voltage_divider-F_Silkscreen.gbr \
    voltage_divider-B_Silkscreen.gbr \
    voltage_divider-F_Paste.gbr \
    voltage_divider-B_Paste.gbr \
    voltage_divider-Edge_Cuts.gbr \
    voltage_divider-job.gbrjob; do
    truncate -s 0 "${root}/gerber/${name}"
  done
  truncate -s 0 "${root}/drill/voltage_divider-NPTH.drl"
  truncate -s 0 "${root}/drill/voltage_divider-PTH.drl"
  truncate -s 0 "${root}/position/voltage_divider-all-pos.csv"
  truncate -s 0 "${root}/receipt/host.json"
}

python3 -c 'import importlib.util,sys; spec=importlib.util.spec_from_file_location("runner",sys.argv[1]); module=importlib.util.module_from_spec(spec); spec.loader.exec_module(module); assert module._checked_aggregate([67108864]*4)==268435456; raised=False
try: module._checked_aggregate([67108864]*4+[1])
except module.HostExportError: raised=True
assert raised' "${runner}"

over_file_root="${TEST_TMPDIR}/over-file-raw"
create_raw_inventory "${over_file_root}"
truncate -s 67108865 "${over_file_root}/gerber/voltage_divider-F_Cu.gbr"
expect_failure \
  "raw input is not a bounded single-link regular file" \
  "${binder}" bind \
  "${source_fixture}" \
  "${catalog_snapshot}" \
  production \
  "${board_fixture}" \
  "${over_file_root}" \
  "${mutating_host}"

over_aggregate_root="${TEST_TMPDIR}/over-aggregate-raw"
create_raw_inventory "${over_aggregate_root}"
for name in \
  voltage_divider-F_Cu.gbr \
  voltage_divider-F_Mask.gbr \
  voltage_divider-B_Cu.gbr \
  voltage_divider-B_Mask.gbr; do
  truncate -s 67108864 "${over_aggregate_root}/gerber/${name}"
done
truncate -s 1 "${over_aggregate_root}/gerber/voltage_divider-F_Silkscreen.gbr"
expect_failure \
  "raw fabrication aggregate exceeds 256 MiB" \
  "${binder}" bind \
  "${source_fixture}" \
  "${catalog_snapshot}" \
  production \
  "${board_fixture}" \
  "${over_aggregate_root}" \
  "${mutating_host}"

real_raw_root="${TEST_TMPDIR}/real-raw"
create_raw_inventory "${real_raw_root}"
ln -s "${real_raw_root}" "${TEST_TMPDIR}/raw-root-link"
expect_failure \
  "failed to open no-follow directory" \
  "${binder}" bind \
  "${source_fixture}" \
  "${catalog_snapshot}" \
  production \
  "${board_fixture}" \
  "${TEST_TMPDIR}/raw-root-link" \
  "${mutating_host}"

child_link_root="${TEST_TMPDIR}/child-link-raw"
mkdir -p "${child_link_root}/drill" "${child_link_root}/position" "${child_link_root}/receipt"
ln -s "${real_raw_root}/gerber" "${child_link_root}/gerber"
expect_failure \
  "failed to open no-follow directory" \
  "${binder}" bind \
  "${source_fixture}" \
  "${catalog_snapshot}" \
  production \
  "${board_fixture}" \
  "${child_link_root}" \
  "${mutating_host}"

hardlink_root="${TEST_TMPDIR}/hardlink-raw"
create_raw_inventory "${hardlink_root}"
unlink "${hardlink_root}/gerber/voltage_divider-F_Mask.gbr"
ln \
  "${hardlink_root}/gerber/voltage_divider-F_Cu.gbr" \
  "${hardlink_root}/gerber/voltage_divider-F_Mask.gbr"
expect_failure \
  "raw input is not a bounded single-link regular file" \
  "${binder}" bind \
  "${source_fixture}" \
  "${catalog_snapshot}" \
  production \
  "${board_fixture}" \
  "${hardlink_root}" \
  "${mutating_host}"
