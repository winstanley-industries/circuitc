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
checked_source_fixture="${10}"
routed_source_fixture="${11}"
raw_fixture_generator="${12}"

# Bazel data dependencies are exposed through a runfiles symlink forest. Stage
# ordinary files before exercising the production gate, which intentionally
# rejects symlinked authenticated inputs.
cp "${source_fixture}" "${TEST_TMPDIR}/fixture.circuitc"
source_fixture="${TEST_TMPDIR}/fixture.circuitc"
cp "${catalog_snapshot}" "${TEST_TMPDIR}/catalog.json"
catalog_snapshot="${TEST_TMPDIR}/catalog.json"
cp "${checked_source_fixture}" "${TEST_TMPDIR}/checked-fixture.circuitc"
checked_source_fixture="${TEST_TMPDIR}/checked-fixture.circuitc"
cp "${routed_source_fixture}" "${TEST_TMPDIR}/routed-fixture.circuitc"
routed_source_fixture="${TEST_TMPDIR}/routed-fixture.circuitc"

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

# The production gate must preserve checked compiler evidence for both reasons
# ADR-0011 requires it: declared simulations and APGAR routing requests.
"${frontend}" compile "${checked_source_fixture}" --output-dir "${TEST_TMPDIR}/checked-compiled"
checked_board="${TEST_TMPDIR}/checked-compiled/checked_voltage_divider.kicad_pcb"
"${binder}" prepare \
  "${checked_source_fixture}" \
  "${catalog_snapshot}" \
  production \
  "${checked_board}" \
  >"${TEST_TMPDIR}/checked-request.json"
"${frontend}" compile "${routed_source_fixture}" --output-dir "${TEST_TMPDIR}/routed-compiled"
routed_board="${TEST_TMPDIR}/routed-compiled/routed_voltage_divider.kicad_pcb"
"${binder}" prepare \
  "${routed_source_fixture}" \
  "${catalog_snapshot}" \
  production \
  "${routed_board}" \
  >"${TEST_TMPDIR}/routed-request.json"

for checked_case in checked routed; do
  if [[ "${checked_case}" == "checked" ]]; then
    checked_source="${checked_source_fixture}"
    checked_design="checked_voltage_divider"
    checked_board_path="${checked_board}"
  else
    checked_source="${routed_source_fixture}"
    checked_design="routed_voltage_divider"
    checked_board_path="${routed_board}"
  fi
  checked_request="${TEST_TMPDIR}/${checked_case}-request.json"
  checked_raw="${TEST_TMPDIR}/${checked_case}-raw"
  python3 "${raw_fixture_generator}" \
    "${checked_raw}" \
    "${checked_design}" \
    "${checked_request}" \
    "${checked_board_path}" \
    "${mutating_host}"
  "${binder}" bind \
    "${checked_source}" \
    "${catalog_snapshot}" \
    production \
    "${checked_board_path}" \
    "${checked_raw}" \
    "${mutating_host}" \
    >"${TEST_TMPDIR}/${checked_case}-manifest.json"
  python3 -c 'import json,pathlib,sys; value=json.loads(pathlib.Path(sys.argv[1]).read_bytes()); assert value["schema_name"]=="circuitc.fabrication_manifest"' \
    "${TEST_TMPDIR}/${checked_case}-manifest.json"
done

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
test ! -e "${TEST_TMPDIR}/wrong-version-work"

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
  chmod 700 "${root}" "${root}/gerber" "${root}/drill" "${root}/position" "${root}/receipt"
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
  chmod 600 \
    "${root}/gerber/"* \
    "${root}/drill/"* \
    "${root}/position/"* \
    "${root}/receipt/"*
}

python3 -c 'import importlib.util,sys; spec=importlib.util.spec_from_file_location("runner",sys.argv[1]); module=importlib.util.module_from_spec(spec); spec.loader.exec_module(module); assert module._checked_aggregate([67108864]*4)==268435456; raised=False
try: module._checked_aggregate([67108864]*4+[1])
except module.HostExportError: raised=True
assert raised' "${runner}"

python3 - "${runner}" "${TEST_TMPDIR}" <<'PY'
import errno
import importlib.util
import os
import pathlib
import sys

spec = importlib.util.spec_from_file_location("runner", sys.argv[1])
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)
root = pathlib.Path(sys.argv[2]) / "namespace-hardening"
root.mkdir(mode=0o700)

unsafe_work = root / "unsafe-work"
unsafe_work.mkdir(mode=0o700)
unsafe_work.chmod(0o777)
try:
    runner.PrivateTransaction(unsafe_work)
except runner.HostExportError as error:
    assert "unsafe shared writes" in str(error) or "private directory" in str(error)
else:
    raise AssertionError("shared-write work directory was accepted")

unsafe_output_parent = root / "unsafe-output-parent"
unsafe_output_parent.mkdir(mode=0o700)
unsafe_output_parent.chmod(0o777)
try:
    runner._publish_exact(unsafe_output_parent / "output", {"receipt/host.json": b"{}\n"})
except runner.HostExportError as error:
    assert "unsafe shared writes" in str(error) or "private directory" in str(error)
else:
    raise AssertionError("shared-write output parent was accepted")

cleanup_work = root / "cleanup-work"
transaction = runner.PrivateTransaction(cleanup_work)
(transaction.path / "nested").mkdir(mode=0o700)
(transaction.path / "nested" / "snapshot").write_bytes(b"authentic")
(transaction.path / "snapshot").write_bytes(b"authentic")
transaction.cleanup()
transaction.close()
assert not cleanup_work.exists()

replacement_work = root / "replacement-work"
replacement_work.mkdir(mode=0o700)
transaction = runner.PrivateTransaction(replacement_work)
stolen = replacement_work / f"{transaction.name}.stolen"
transaction.path.rename(stolen)
transaction.path.mkdir(mode=0o700)
(transaction.path / "replacement-marker").write_bytes(b"replacement")
for operation in (transaction.recheck, transaction.cleanup):
    try:
        operation()
    except runner.HostExportError as error:
        assert "name no longer identifies" in str(error)
    else:
        raise AssertionError("replaced transaction name was accepted")
assert (transaction.path / "replacement-marker").read_bytes() == b"replacement"
assert stolen.is_dir()
transaction.close()

process_work = root / "process-boundary-work"
process_work.mkdir(mode=0o700)
transaction = runner.PrivateTransaction(process_work)
(transaction.path / "authentic-snapshot").write_bytes(b"authentic")
process_board = root / "process-board"
process_executable = root / "process-executable"
process_board.write_bytes(b"board")
process_executable.write_bytes(b"executable")
board_fd, board_identity = runner._open_regular(process_board, 1024)
executable_fd, executable_identity = runner._open_regular(process_executable, 1024)
original_run_process = runner._run_process
process_stolen = process_work / f"{transaction.name}.stolen"

def replace_during_process(_command, _environment, _label):
    transaction.path.rename(process_stolen)
    transaction.path.mkdir(mode=0o700)
    (transaction.path / "attacker-marker").write_bytes(b"must-not-be-executed-or-deleted")
    return b"", b""

runner._run_process = replace_during_process
try:
    runner._run(
        ["unused"],
        {},
        "replacement boundary",
        board_fd,
        board_identity,
        process_board,
        executable_fd,
        executable_identity,
        process_executable,
        transaction,
    )
except runner.HostExportError as error:
    assert "name no longer identifies" in str(error)
else:
    raise AssertionError("post-command transaction replacement was accepted")
finally:
    runner._run_process = original_run_process
    os.close(board_fd)
    os.close(executable_fd)
try:
    transaction.cleanup()
except runner.HostExportError as error:
    assert "name no longer identifies" in str(error)
else:
    raise AssertionError("cleanup crossed a replaced process-boundary name")
assert (transaction.path / "attacker-marker").read_bytes() == b"must-not-be-executed-or-deleted"
assert (process_stolen / "authentic-snapshot").read_bytes() == b"authentic"
transaction.close()

original_fchmod = runner.os.fchmod

def fail_fchmod(_descriptor, _mode):
    raise OSError(errno.EIO, "simulated constructor fchmod failure")

created_work = root / "constructor-created-work"
runner.os.fchmod = fail_fchmod
try:
    runner.PrivateTransaction(created_work)
except runner.HostExportError as error:
    assert "simulated constructor fchmod failure" in str(error)
else:
    raise AssertionError("work-directory constructor failure was accepted")
finally:
    runner.os.fchmod = original_fchmod
assert not created_work.exists()

fd_directory = pathlib.Path("/proc/self/fd")
if not fd_directory.is_dir():
    fd_directory = pathlib.Path("/dev/fd")
before_parent_failure_fds = len(list(fd_directory.iterdir()))
missing_parent_work = root / "constructor-new-parent" / "work"
runner.os.fchmod = fail_fchmod
try:
    runner.PrivateTransaction(missing_parent_work)
except runner.HostExportError as error:
    assert "simulated constructor fchmod failure" in str(error)
else:
    raise AssertionError("work-parent constructor failure was accepted")
finally:
    runner.os.fchmod = original_fchmod
assert not missing_parent_work.parent.exists()
assert len(list(fd_directory.iterdir())) == before_parent_failure_fds

original_umask = os.umask(0o200)
fchmod_calls = {"count": 0}

def fail_second_parent_fchmod(descriptor, mode):
    fchmod_calls["count"] += 1
    if fchmod_calls["count"] == 2:
        raise OSError(errno.EIO, "simulated nested-parent fchmod failure")
    return original_fchmod(descriptor, mode)

nested_parent_work = root / "constructor-created-a" / "constructor-created-b" / "work"
runner.os.fchmod = fail_second_parent_fchmod
try:
    runner.PrivateTransaction(nested_parent_work)
except runner.HostExportError as error:
    assert "simulated nested-parent fchmod failure" in str(error)
else:
    raise AssertionError("nested work-parent constructor failure was accepted")
finally:
    runner.os.fchmod = original_fchmod
    os.umask(original_umask)
assert not (root / "constructor-created-a").exists()

fchmod_calls = {"count": 0}

def fail_terminal_work_fchmod(descriptor, mode):
    fchmod_calls["count"] += 1
    if fchmod_calls["count"] == 2:
        raise OSError(errno.EIO, "simulated terminal-work fchmod failure")
    return original_fchmod(descriptor, mode)

downstream_work = root / "constructor-downstream-parent" / "work"
before_downstream_failure_fds = len(list(fd_directory.iterdir()))
runner.os.fchmod = fail_terminal_work_fchmod
try:
    runner.PrivateTransaction(downstream_work)
except runner.HostExportError as error:
    assert "simulated terminal-work fchmod failure" in str(error)
else:
    raise AssertionError("downstream constructor failure was accepted")
finally:
    runner.os.fchmod = original_fchmod
assert not downstream_work.parent.exists()
assert len(list(fd_directory.iterdir())) == before_downstream_failure_fds

original_directory_identity = runner._directory_identity

def fail_parent_identity(_metadata):
    raise OSError(errno.EIO, "simulated held-parent identity failure")

before_private_identity_fds = len(list(fd_directory.iterdir()))
runner._directory_identity = fail_parent_identity
try:
    runner.PrivateTransaction(root / "identity-failure-work")
except OSError as error:
    assert "simulated held-parent identity failure" in str(error)
else:
    raise AssertionError("held work-parent identity failure was accepted")
finally:
    runner._directory_identity = original_directory_identity
assert len(list(fd_directory.iterdir())) == before_private_identity_fds

existing_work = root / "constructor-existing-work"
existing_work.mkdir(mode=0o700)
runner.os.fchmod = fail_fchmod
try:
    runner.PrivateTransaction(existing_work)
except runner.HostExportError as error:
    assert "simulated constructor fchmod failure" in str(error)
else:
    raise AssertionError("transaction constructor failure was accepted")
finally:
    runner.os.fchmod = original_fchmod
assert list(existing_work.iterdir()) == []

publish_parent = root / "publish-parent"
publish_parent.mkdir(mode=0o700)

before_publish_identity_fds = len(list(fd_directory.iterdir()))
runner._directory_identity = fail_parent_identity
try:
    runner._publish_exact(
        publish_parent / "identity-failure-output", {"receipt/host.json": b"failure\n"}
    )
except runner.HostExportError as error:
    assert "simulated held-parent identity failure" in str(error)
else:
    raise AssertionError("held publication-parent identity failure was accepted")
finally:
    runner._directory_identity = original_directory_identity
assert len(list(fd_directory.iterdir())) == before_publish_identity_fds

probe_parent_fd = os.open(publish_parent, runner._directory_flags())
original_open = runner.os.open

def fail_named_probe_open(path, flags, *args, **kwargs):
    if path == "transport-error-probe":
        raise OSError(errno.EIO, "simulated name-open transport failure")
    return original_open(path, flags, *args, **kwargs)

runner.os.open = fail_named_probe_open
try:
    transport_probe = runner._probe_named_directory(probe_parent_fd, "transport-error-probe")
finally:
    runner.os.open = original_open
    os.close(probe_parent_fd)
assert transport_probe.kind is runner._ProbeKind.ERROR
assert transport_probe.error is not None
assert transport_probe.error.errno == errno.EIO

early_output = publish_parent / "early-output"
runner.os.fchmod = fail_fchmod
try:
    runner._publish_exact(early_output, {"receipt/host.json": b"early\n"})
except runner.HostExportError as error:
    assert "simulated constructor fchmod failure" in str(error)
else:
    raise AssertionError("publication constructor failure was accepted")
finally:
    runner.os.fchmod = original_fchmod
assert not early_output.exists()
assert list(publish_parent.glob(".early-output.circuitc-*")) == []

committed_output = publish_parent / "committed-output"
original_rename = runner._rename_noreplace

def commit_then_error(parent_fd, source, destination):
    os.rename(
        source,
        destination,
        src_dir_fd=parent_fd,
        dst_dir_fd=parent_fd,
    )
    raise OSError(errno.EIO, "simulated post-commit error")

runner._rename_noreplace = commit_then_error
runner._publish_exact(committed_output, {"receipt/host.json": b"committed\n"})
assert (committed_output / "receipt/host.json").read_bytes() == b"committed\n"

replaced_output = publish_parent / "replaced-output"

def replace_then_error(parent_fd, source, destination):
    os.rename(
        source,
        f"{source}.stolen",
        src_dir_fd=parent_fd,
        dst_dir_fd=parent_fd,
    )
    os.mkdir(source, mode=0o700, dir_fd=parent_fd)
    replacement_fd = os.open(source, runner._directory_flags(), dir_fd=parent_fd)
    try:
        marker = os.open(
            "replacement-marker",
            os.O_WRONLY | os.O_CREAT | os.O_EXCL,
            0o600,
            dir_fd=replacement_fd,
        )
        os.close(marker)
    finally:
        os.close(replacement_fd)
    raise OSError(errno.EIO, "simulated replaced source name")

runner._rename_noreplace = replace_then_error
try:
    runner._publish_exact(replaced_output, {"receipt/host.json": b"retained\n"})
except runner.HostExportError as error:
    assert "indeterminate visibility" in str(error)
else:
    raise AssertionError("ambiguous publication was accepted")
assert not replaced_output.exists()
temporary = next(
    path
    for path in publish_parent.glob(".replaced-output.circuitc-*")
    if not path.name.endswith(".stolen")
)
assert (temporary / "replacement-marker").is_file()
authentic = next(publish_parent.glob(".replaced-output.circuitc-*.stolen"))
assert (authentic / "receipt/host.json").read_bytes() == b"retained\n"

def error_without_rename(_parent_fd, _source, _destination):
    raise OSError(errno.EIO, "simulated ambiguous rename")

runner._rename_noreplace = error_without_rename
not_found_output = publish_parent / "source-positive-final-not-found"
try:
    runner._publish_exact(not_found_output, {"receipt/host.json": b"not-found\n"})
except runner.HostExportError as error:
    assert "indeterminate visibility" in str(error)
    assert "final not found" in str(error)
else:
    raise AssertionError("source-positive/final-not-found rename was accepted")
not_found_residue = next(
    publish_parent.glob(".source-positive-final-not-found.circuitc-*")
)
assert (not_found_residue / "receipt/host.json").read_bytes() == b"not-found\n"

probe_error_output = publish_parent / "source-positive-final-eio"
original_probe = runner._probe_named_directory
probe_state = {"rename_called": False}

def error_before_final_probe(_parent_fd, _source, _destination):
    probe_state["rename_called"] = True
    raise OSError(errno.EIO, "simulated ambiguous rename")

def final_probe_eio(parent_fd, name):
    if probe_state["rename_called"] and name == probe_error_output.name:
        return runner._NameProbe(
            runner._ProbeKind.ERROR,
            error=OSError(errno.EIO, "simulated final-name probe failure"),
        )
    return original_probe(parent_fd, name)

runner._rename_noreplace = error_before_final_probe
runner._probe_named_directory = final_probe_eio
try:
    runner._publish_exact(probe_error_output, {"receipt/host.json": b"probe-error\n"})
except runner.HostExportError as error:
    assert "indeterminate visibility" in str(error)
    assert "simulated final-name probe failure" in str(error)
else:
    raise AssertionError("source-positive/final-EIO rename was accepted")
finally:
    runner._probe_named_directory = original_probe
probe_error_residue = next(
    publish_parent.glob(".source-positive-final-eio.circuitc-*")
)
assert (probe_error_residue / "receipt/host.json").read_bytes() == b"probe-error\n"

for rejection_errno in sorted(
    {errno.EINVAL, errno.ENOSYS, errno.ENOTSUP, errno.EOPNOTSUPP}
):
    rejected_output = publish_parent / f"rejected-{rejection_errno}"

    def reject_before_execution(_parent_fd, _source, _destination, value=rejection_errno):
        raise OSError(value, "simulated primitive rejection")

    runner._rename_noreplace = reject_before_execution
    try:
        runner._publish_exact(rejected_output, {"receipt/host.json": b"rejected\n"})
    except runner._RenameUnsupported as error:
        assert "rejected" in str(error)
    else:
        raise AssertionError(f"rename rejection {rejection_errno} was accepted")
    assert not rejected_output.exists()
    assert list(publish_parent.glob(f".rejected-{rejection_errno}.circuitc-*")) == []

runner._rename_noreplace = original_rename
PY

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
chmod 700 "${child_link_root}" "${child_link_root}/drill" "${child_link_root}/position" "${child_link_root}/receipt"
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

permissive_root="${TEST_TMPDIR}/permissive-root-raw"
create_raw_inventory "${permissive_root}"
chmod 755 "${permissive_root}"
expect_failure \
  "raw fabrication directory is not effective-uid-owned and private" \
  "${binder}" bind \
  "${source_fixture}" \
  "${catalog_snapshot}" \
  production \
  "${board_fixture}" \
  "${permissive_root}" \
  "${mutating_host}"

permissive_child="${TEST_TMPDIR}/permissive-child-raw"
create_raw_inventory "${permissive_child}"
chmod 755 "${permissive_child}/gerber"
expect_failure \
  "raw fabrication directory is not effective-uid-owned and private: gerber" \
  "${binder}" bind \
  "${source_fixture}" \
  "${catalog_snapshot}" \
  production \
  "${board_fixture}" \
  "${permissive_child}" \
  "${mutating_host}"

permissive_file="${TEST_TMPDIR}/permissive-file-raw"
create_raw_inventory "${permissive_file}"
chmod 644 "${permissive_file}/receipt/host.json"
expect_failure \
  "raw input is not effective-uid-owned and private" \
  "${binder}" bind \
  "${source_fixture}" \
  "${catalog_snapshot}" \
  production \
  "${board_fixture}" \
  "${permissive_file}" \
  "${mutating_host}"
