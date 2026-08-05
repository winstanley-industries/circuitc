#!/bin/bash
set -euo pipefail

runner_source="$1"
fake_kicad_source="$2"
fake_host_source="$3"
fake_over_source="$4"
fake_exact_source="$5"
request_writer_source="$6"
request_mutator_source="$7"
fake_normalizer_source="$8"

cp "${runner_source}" "${TEST_TMPDIR}/runner.py"
runner="${TEST_TMPDIR}/runner.py"
chmod 0755 "${runner}"
cp "${fake_kicad_source}" "${TEST_TMPDIR}/kicad-cli"
fake_kicad="${TEST_TMPDIR}/kicad-cli"
chmod 0755 "${fake_kicad}"
cp "${fake_host_source}" "${TEST_TMPDIR}/host.py"
fake_host="${TEST_TMPDIR}/host.py"
cp "${fake_over_source}" "${TEST_TMPDIR}/host-over.py"
fake_over="${TEST_TMPDIR}/host-over.py"
cp "${fake_exact_source}" "${TEST_TMPDIR}/host-exact.py"
fake_exact="${TEST_TMPDIR}/host-exact.py"
cp "${request_writer_source}" "${TEST_TMPDIR}/make-request.py"
request_writer="${TEST_TMPDIR}/make-request.py"
cp "${request_mutator_source}" "${TEST_TMPDIR}/mutate-request.py"
request_mutator="${TEST_TMPDIR}/mutate-request.py"
cp "${fake_normalizer_source}" "${TEST_TMPDIR}/normalizer.py"
normalizer="${TEST_TMPDIR}/normalizer.py"

project="${TEST_TMPDIR}/project"
mkdir -p "${project}"
mkdir -p "${project}/CircuitC.pretty"
printf 'schematic\n' >"${project}/voltage_divider.kicad_sch"
printf 'board\n' >"${project}/voltage_divider.kicad_pcb"
printf '{"schema_version":1}\n' >"${project}/voltage_divider.kicad-map.json"
printf 'project\n' >"${project}/voltage_divider.kicad_pro"
printf 'symbols\n' >"${project}/CircuitC.kicad_sym"
printf 'footprint\n' \
  >"${project}/CircuitC.pretty/R_0603_1608Metric.kicad_mod"
printf 'symbol table\n' >"${project}/sym-lib-table"
printf 'footprint table\n' >"${project}/fp-lib-table"
printf '{"schema_name":"test.fabrication"}\n' >"${TEST_TMPDIR}/fabrication-manifest.json"

hostile_python="${TEST_TMPDIR}/hostile-python"
hostile_marker="${TEST_TMPDIR}/sitecustomize-ran"
mkdir -p "${hostile_python}"
python3 - "${hostile_python}/sitecustomize.py" "${hostile_marker}" <<'PY'
import pathlib
import sys

pathlib.Path(sys.argv[1]).write_text(
    "import pathlib\npathlib.Path(" + repr(sys.argv[2]) + ").write_text('ran')\n",
    encoding="utf-8",
)
PY

python3 "${request_writer}" \
  --project "${project}" \
  --fabrication-manifest "${TEST_TMPDIR}/fabrication-manifest.json" \
  --output "${TEST_TMPDIR}/request.json"

PYTHONPATH="${hostile_python}" "${runner}" \
  --request "${TEST_TMPDIR}/request.json" \
  --project-dir "${project}" \
  --fabrication-manifest "${TEST_TMPDIR}/fabrication-manifest.json" \
  --kicad-cli "${fake_kicad}" \
  --normalizer "${normalizer}" \
  --host-runner "${fake_host}" \
  --work-dir "${TEST_TMPDIR}/work" \
  --output-root "${TEST_TMPDIR}/output"
test ! -e "${hostile_marker}"

test -f "${TEST_TMPDIR}/output/erc.normalized.json"
test -f "${TEST_TMPDIR}/output/drc.normalized.json"
test -f "${TEST_TMPDIR}/output/receipt.json"

printf 'ambient unbound design rules\n' >"${project}/voltage_divider.kicad_dru"
"${runner}" \
  --request "${TEST_TMPDIR}/request.json" \
  --project-dir "${project}" \
  --fabrication-manifest "${TEST_TMPDIR}/fabrication-manifest.json" \
  --kicad-cli "${fake_kicad}" \
  --normalizer "${normalizer}" \
  --host-runner "${fake_host}" \
  --work-dir "${TEST_TMPDIR}/work" \
  --output-root "${TEST_TMPDIR}/ambient-output"
cmp "${TEST_TMPDIR}/output/erc.normalized.json" \
  "${TEST_TMPDIR}/ambient-output/erc.normalized.json"
cmp "${TEST_TMPDIR}/output/drc.normalized.json" \
  "${TEST_TMPDIR}/ambient-output/drc.normalized.json"
cmp "${TEST_TMPDIR}/output/receipt.json" \
  "${TEST_TMPDIR}/ambient-output/receipt.json"

"${runner}" \
  --request "${TEST_TMPDIR}/request.json" \
  --project-dir "${project}" \
  --fabrication-manifest "${TEST_TMPDIR}/fabrication-manifest.json" \
  --kicad-cli "${fake_kicad}" \
  --normalizer "${normalizer}" \
  --host-runner "${fake_exact}" \
  --work-dir "${TEST_TMPDIR}/work" \
  --output-root "${TEST_TMPDIR}/exact-stdout-output"

python3 -I - "${runner}" "${TEST_TMPDIR}" <<'PY'
import errno
import importlib.util
import os
import pathlib
import stat
import sys
import tempfile
import types

spec = importlib.util.spec_from_file_location("analysis_runner", sys.argv[1])
runner = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(runner)
environment = {"LC_ALL": "C", "PATH": "/usr/bin:/bin"}

untrusted_directory = types.SimpleNamespace(
    st_mode=stat.S_IFDIR | 0o755,
    st_uid=os.geteuid() + 1,
)
try:
    runner._validate_directory(
        untrusted_directory,
        pathlib.Path("untrusted-ancestor"),
        protected_namespace=True,
        private_terminal=False,
    )
except runner.AnalysisHostError:
    pass
else:
    raise AssertionError("accepted a protected ancestor owned by an untrusted uid")

exact = "import sys; sys.stdout.buffer.write(b'x' * 1048576); sys.stderr.buffer.write(b'y' * 1048576)"
stdout, stderr = runner._run_process(
    [sys.executable, "-I", "-c", exact], 2.0, "exact stdio", environment
)
assert len(stdout) == 1048576
assert len(stderr) == 1048576

for stream in ("stdout", "stderr"):
    script = f"import sys; sys.{stream}.buffer.write(b'x' * 1048577)"
    try:
        runner._run_process(
            [sys.executable, "-I", "-c", script], 2.0, f"{stream} over", environment
        )
    except runner.AnalysisHostError:
        pass
    else:
        raise AssertionError(f"accepted {stream} one byte over its limit")

try:
    runner._run_process(
        [sys.executable, "-I", "-c", "import time; time.sleep(2)"],
        0.05,
        "timeout",
        environment,
    )
except runner.AnalysisHostError:
    pass
else:
    raise AssertionError("accepted a process beyond its deadline")

with tempfile.TemporaryDirectory(dir=sys.argv[2]) as directory:
    boundary = pathlib.Path(directory) / "boundary.bin"
    with boundary.open("wb") as output:
        output.truncate(runner.MAX_FILE_BYTES)
    data, _ = runner._read_bounded(boundary)
    assert len(data) == runner.MAX_FILE_BYTES
    del data
    with boundary.open("r+b") as output:
        output.truncate(runner.MAX_FILE_BYTES + 1)
    try:
        runner._read_bounded(boundary)
    except runner.AnalysisHostError:
        pass
    else:
        raise AssertionError("accepted a file one byte over its limit")

publication_outputs = {
    "drc.normalized.json": b'{"drc":"exact"}\n',
    "erc.normalized.json": b'{"erc":"exact"}\n',
    "receipt.json": b'{"receipt":"exact"}\n',
}


def assert_exact_flat_tree(root, expected):
    actual = {entry.name: entry.read_bytes() for entry in root.iterdir()}
    assert actual == expected


with tempfile.TemporaryDirectory(dir=sys.argv[2]) as directory:
    parent = pathlib.Path(directory)
    output_root = parent / "committed-after-error"
    original_rename = runner._rename_noreplace

    def commit_then_eio(parent_fd, source, destination):
        original_rename(parent_fd, source, destination)
        raise OSError(errno.EIO, os.strerror(errno.EIO))

    runner._rename_noreplace = commit_then_eio
    try:
        runner._publish(output_root, publication_outputs)
    finally:
        runner._rename_noreplace = original_rename
    assert_exact_flat_tree(output_root, publication_outputs)
    assert not list(parent.glob(f".{output_root.name}.circuitc-*"))

with tempfile.TemporaryDirectory(dir=sys.argv[2]) as directory:
    parent = pathlib.Path(directory)
    output_root = parent / "different-final-output"
    original_rename = runner._rename_noreplace

    def install_different_final(parent_fd, _source, destination):
        os.mkdir(destination, mode=0o700, dir_fd=parent_fd)
        final_fd = os.open(destination, runner._directory_flags(), dir_fd=parent_fd)
        try:
            marker = os.open(
                "different-final-marker",
                os.O_WRONLY | os.O_CREAT | os.O_EXCL,
                0o600,
                dir_fd=final_fd,
            )
            os.close(marker)
        finally:
            os.close(final_fd)
        raise OSError(errno.EEXIST, os.strerror(errno.EEXIST))

    runner._rename_noreplace = install_different_final
    try:
        try:
            runner._publish(output_root, publication_outputs)
        except runner.AnalysisHostError as error:
            assert "already exists" in str(error)
        else:
            raise AssertionError("accepted a different final inode after rename failure")
    finally:
        runner._rename_noreplace = original_rename
    assert (output_root / "different-final-marker").is_file()
    assert not list(parent.glob(f".{output_root.name}.circuitc-*"))

for rejection_errno in sorted(runner._PREEXECUTION_RENAME_ERRORS):
    with tempfile.TemporaryDirectory(dir=sys.argv[2]) as directory:
        parent = pathlib.Path(directory)
        output_root = parent / f"rejected-{rejection_errno}"
        original_rename = runner._rename_noreplace

        def reject_before_execution(
            _parent_fd, _source, _destination, value=rejection_errno
        ):
            raise OSError(value, os.strerror(value))

        runner._rename_noreplace = reject_before_execution
        try:
            try:
                runner._publish(output_root, publication_outputs)
            except runner.AnalysisHostError as error:
                assert "no atomic no-replace" in str(error)
            else:
                raise AssertionError(f"accepted primitive rejection {rejection_errno}")
        finally:
            runner._rename_noreplace = original_rename
        assert not output_root.exists()
        assert not list(parent.glob(f".{output_root.name}.circuitc-*"))

for source_probe in ("stale-identity", "io-error"):
    with tempfile.TemporaryDirectory(dir=sys.argv[2]) as directory:
        parent = pathlib.Path(directory)
        output_root = parent / f"committed-inconclusive-{source_probe}"
        original_cleanup = runner._cleanup_staged_publication
        original_rename = runner._rename_noreplace
        original_stat = runner.os.stat
        rename_state = {}

        def reject_cleanup(*_args):
            raise AssertionError("cleanup ran after an indeterminate committed rename")

        def commit_then_eio(parent_fd, source, destination):
            rename_state["source"] = source
            rename_state["metadata"] = original_stat(
                source, dir_fd=parent_fd, follow_symlinks=False
            )
            original_rename(parent_fd, source, destination)
            raise OSError(errno.EIO, os.strerror(errno.EIO))

        def spoof_source_probe(path, *args, **kwargs):
            if (
                path == rename_state.get("source")
                and kwargs.get("dir_fd") is not None
            ):
                if source_probe == "stale-identity":
                    return rename_state["metadata"]
                raise OSError(errno.EIO, os.strerror(errno.EIO))
            return original_stat(path, *args, **kwargs)

        runner._cleanup_staged_publication = reject_cleanup
        runner._rename_noreplace = commit_then_eio
        runner.os.stat = spoof_source_probe
        try:
            try:
                runner._publish(output_root, publication_outputs)
            except runner.AnalysisHostError as error:
                assert "publication visibility is indeterminate" in str(error)
            else:
                raise AssertionError("accepted an inconclusive source-name probe")
        finally:
            runner.os.stat = original_stat
            runner._rename_noreplace = original_rename
            runner._cleanup_staged_publication = original_cleanup
        assert_exact_flat_tree(output_root, publication_outputs)
        assert not list(parent.glob(f".{output_root.name}.circuitc-*"))

for probe_errno in (errno.EIO, errno.ENOENT):
    with tempfile.TemporaryDirectory(dir=sys.argv[2]) as directory:
        parent = pathlib.Path(directory)
        output_root = parent / f"inconclusive-{probe_errno}"
        original_rename = runner._rename_noreplace
        original_stat = runner.os.stat

        def fail_before_rename(_parent_fd, _source, _destination):
            raise OSError(errno.EIO, os.strerror(errno.EIO))

        def fail_final_probe(path, *args, **kwargs):
            if path == output_root.name and kwargs.get("dir_fd") is not None:
                raise OSError(probe_errno, os.strerror(probe_errno))
            return original_stat(path, *args, **kwargs)

        runner._rename_noreplace = fail_before_rename
        runner.os.stat = fail_final_probe
        try:
            try:
                runner._publish(output_root, publication_outputs)
            except runner.AnalysisHostError as error:
                assert "publication visibility is indeterminate" in str(error)
            else:
                raise AssertionError("accepted an inconclusive publication probe")
        finally:
            runner.os.stat = original_stat
            runner._rename_noreplace = original_rename
        assert not output_root.exists()
        residue = list(parent.glob(f".{output_root.name}.circuitc-*"))
        assert len(residue) == 1
        assert_exact_flat_tree(residue[0], publication_outputs)
PY

expect_failure() {
  label="$1"
  shift
  output_root=""
  previous=""
  for argument in "$@"; do
    if [[ "${previous}" == "--output-root" ]]; then
      output_root="${argument}"
      break
    fi
    previous="${argument}"
  done
  if "$@" >"${TEST_TMPDIR}/${label}.stdout" 2>"${TEST_TMPDIR}/${label}.stderr"; then
    echo "expected failure for ${label}" >&2
    exit 1
  fi
  if [[ "${label}" != "no_replace" && -n "${output_root}" && -e "${output_root}" ]]; then
    echo "failed analysis published output for ${label}" >&2
    exit 1
  fi
  if find "${TEST_TMPDIR}/work" \
    \( -name 'analysis-*' -o -name 'circuitc-kicad-*' \) \
    -print -quit 2>/dev/null | grep -q .; then
    echo "failed analysis retained a transaction for ${label}" >&2
    exit 1
  fi
  if find "${TEST_TMPDIR}" -name '.*.circuitc-*' -print -quit | grep -q .; then
    echo "failed analysis retained a publication temporary for ${label}" >&2
    exit 1
  fi
}

python3 -I - \
  "${runner}" \
  "${TEST_TMPDIR}/request.json" \
  "${project}" \
  "${TEST_TMPDIR}/fabrication-manifest.json" \
  "${fake_kicad}" \
  "${normalizer}" \
  "${fake_host}" \
  "${TEST_TMPDIR}" <<'PY'
import argparse
import importlib.util
import pathlib
import sys

spec = importlib.util.spec_from_file_location("analysis_runner", sys.argv[1])
runner = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(runner)
request, project, fabrication, executable, normalizer, host, root = map(
    pathlib.Path, sys.argv[2:]
)


def args(label):
    return argparse.Namespace(
        request=request,
        project_dir=project,
        fabrication_manifest=fabrication,
        kicad_cli=executable,
        normalizer=normalizer,
        host_runner=host,
        work_dir=root / f"snapshot-{label}-work",
        output_root=root / f"snapshot-{label}-output",
    )


def prove_snapshot(target, label, attacker):
    original = target.read_bytes()
    original_mode = target.stat().st_mode & 0o777
    marker = root / f"{label}-attacker-ran"
    original_run_process = runner._run_process
    changed = False

    def mutate_before_first_spawn(command, timeout_seconds, process_label, environment):
        nonlocal changed
        if not changed:
            target.write_text(attacker.format(marker=str(marker)), encoding="utf-8")
            target.chmod(0o755)
            changed = True
        return original_run_process(command, timeout_seconds, process_label, environment)

    runner._run_process = mutate_before_first_spawn
    try:
        try:
            runner.run(args(label))
        except (OSError, runner.AnalysisHostError):
            pass
        else:
            raise AssertionError("caller-path mutation was not detected")
    finally:
        runner._run_process = original_run_process
        target.write_bytes(original)
        target.chmod(original_mode)
    assert not marker.exists(), f"unauthenticated {label} bytes executed"


prove_snapshot(
    executable,
    "executable",
    "#!/usr/bin/env python3\nimport pathlib\npathlib.Path({marker!r}).write_text('ran')\nprint('10.0.5')\n",
)
prove_snapshot(
    host,
    "host-runner",
    "#!/usr/bin/env python3\nimport pathlib\npathlib.Path({marker!r}).write_text('ran')\n",
)
prove_snapshot(
    normalizer,
    "normalizer",
    "#!/usr/bin/env python3\nimport pathlib\npathlib.Path({marker!r}).write_text('ran')\n",
)
PY

python3 -I - \
  "${runner}" \
  "${TEST_TMPDIR}/request.json" \
  "${project}" \
  "${TEST_TMPDIR}/fabrication-manifest.json" \
  "${fake_kicad}" \
  "${normalizer}" \
  "${fake_host}" \
  "${TEST_TMPDIR}/output" \
  "${TEST_TMPDIR}" <<'PY'
import argparse
import importlib.util
import json
import pathlib
import sys

spec = importlib.util.spec_from_file_location("analysis_runner", sys.argv[1])
runner = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(runner)
request_path, project, fabrication, executable, normalizer, host, baseline, root = map(
    pathlib.Path, sys.argv[2:]
)
request = json.loads(request_path.read_text(encoding="utf-8"))
input_total = sum(
    path.stat().st_size
    for path in [
        request_path,
        project / request["kicad_schematic"]["path"],
        project / request["kicad_pcb"]["path"],
        project / request["kicad_identity_map"]["path"],
        *[project / binding["path"] for binding in request["project_support"]],
        fabrication,
        executable,
        normalizer,
        host,
    ]
)
exact_total = input_total + sum(
    (baseline / name).stat().st_size
    for name in ("erc.normalized.json", "drc.normalized.json", "receipt.json")
)


def args(label):
    return argparse.Namespace(
        request=request_path,
        project_dir=project,
        fabrication_manifest=fabrication,
        kicad_cli=executable,
        normalizer=normalizer,
        host_runner=host,
        work_dir=root / f"aggregate-{label}-work",
        output_root=root / f"aggregate-{label}-output",
    )


runner.MAX_AGGREGATE_BYTES = exact_total
runner.run(args("exact"))
runner.MAX_AGGREGATE_BYTES = exact_total - 1
try:
    runner.run(args("one-over"))
except runner.AnalysisHostError:
    pass
else:
    raise AssertionError("accepted aggregate output one byte over its limit")
assert not (root / "aggregate-one-over-output").exists()
PY

unsafe_work="${TEST_TMPDIR}/unsafe-work"
mkdir -p "${unsafe_work}"
chmod 0777 "${unsafe_work}"
expect_failure unsafe_work "${runner}" \
  --request "${TEST_TMPDIR}/request.json" \
  --project-dir "${project}" \
  --fabrication-manifest "${TEST_TMPDIR}/fabrication-manifest.json" \
  --kicad-cli "${fake_kicad}" \
  --normalizer "${normalizer}" \
  --host-runner "${fake_host}" \
  --work-dir "${unsafe_work}" \
  --output-root "${TEST_TMPDIR}/unsafe-work-output"

unsafe_output_parent="${TEST_TMPDIR}/unsafe-output-parent"
mkdir -p "${unsafe_output_parent}"
chmod 0777 "${unsafe_output_parent}"
expect_failure unsafe_output_parent "${runner}" \
  --request "${TEST_TMPDIR}/request.json" \
  --project-dir "${project}" \
  --fabrication-manifest "${TEST_TMPDIR}/fabrication-manifest.json" \
  --kicad-cli "${fake_kicad}" \
  --normalizer "${normalizer}" \
  --host-runner "${fake_host}" \
  --work-dir "${TEST_TMPDIR}/work" \
  --output-root "${unsafe_output_parent}/output"

expect_failure no_replace "${runner}" \
  --request "${TEST_TMPDIR}/request.json" \
  --project-dir "${project}" \
  --fabrication-manifest "${TEST_TMPDIR}/fabrication-manifest.json" \
  --kicad-cli "${fake_kicad}" \
  --normalizer "${normalizer}" \
  --host-runner "${fake_host}" \
  --work-dir "${TEST_TMPDIR}/work" \
  --output-root "${TEST_TMPDIR}/output"

python3 "${request_mutator}" \
  --input "${TEST_TMPDIR}/request.json" \
  --output "${TEST_TMPDIR}/reordered.json" \
  --mode reorder-policy
expect_failure reordered "${runner}" \
  --request "${TEST_TMPDIR}/reordered.json" \
  --project-dir "${project}" \
  --fabrication-manifest "${TEST_TMPDIR}/fabrication-manifest.json" \
  --kicad-cli "${fake_kicad}" \
  --normalizer "${normalizer}" \
  --host-runner "${fake_host}" \
  --work-dir "${TEST_TMPDIR}/work" \
  --output-root "${TEST_TMPDIR}/reordered-output"

python3 "${request_mutator}" \
  --input "${TEST_TMPDIR}/request.json" \
  --output "${TEST_TMPDIR}/boolean.json" \
  --mode boolean-resource
expect_failure boolean_type "${runner}" \
  --request "${TEST_TMPDIR}/boolean.json" \
  --project-dir "${project}" \
  --fabrication-manifest "${TEST_TMPDIR}/fabrication-manifest.json" \
  --kicad-cli "${fake_kicad}" \
  --normalizer "${normalizer}" \
  --host-runner "${fake_host}" \
  --work-dir "${TEST_TMPDIR}/work" \
  --output-root "${TEST_TMPDIR}/boolean-output"

symlink_project="${TEST_TMPDIR}/symlink-project"
cp -R "${project}" "${symlink_project}"
rm "${symlink_project}/voltage_divider.kicad_pcb"
ln -s "${project}/voltage_divider.kicad_pcb" \
  "${symlink_project}/voltage_divider.kicad_pcb"
expect_failure symlink "${runner}" \
  --request "${TEST_TMPDIR}/request.json" \
  --project-dir "${symlink_project}" \
  --fabrication-manifest "${TEST_TMPDIR}/fabrication-manifest.json" \
  --kicad-cli "${fake_kicad}" \
  --normalizer "${normalizer}" \
  --host-runner "${fake_host}" \
  --work-dir "${TEST_TMPDIR}/work" \
  --output-root "${TEST_TMPDIR}/symlink-output"

ln -s "${project}" "${TEST_TMPDIR}/symlink-project-root"
expect_failure symlink_root "${runner}" \
  --request "${TEST_TMPDIR}/request.json" \
  --project-dir "${TEST_TMPDIR}/symlink-project-root" \
  --fabrication-manifest "${TEST_TMPDIR}/fabrication-manifest.json" \
  --kicad-cli "${fake_kicad}" \
  --normalizer "${normalizer}" \
  --host-runner "${fake_host}" \
  --work-dir "${TEST_TMPDIR}/work" \
  --output-root "${TEST_TMPDIR}/symlink-root-output"

intermediate_project="${TEST_TMPDIR}/intermediate-project"
cp -R "${project}" "${intermediate_project}"
rm -r "${intermediate_project}/CircuitC.pretty"
ln -s "${project}/CircuitC.pretty" "${intermediate_project}/CircuitC.pretty"
expect_failure symlink_intermediate "${runner}" \
  --request "${TEST_TMPDIR}/request.json" \
  --project-dir "${intermediate_project}" \
  --fabrication-manifest "${TEST_TMPDIR}/fabrication-manifest.json" \
  --kicad-cli "${fake_kicad}" \
  --normalizer "${normalizer}" \
  --host-runner "${fake_host}" \
  --work-dir "${TEST_TMPDIR}/work" \
  --output-root "${TEST_TMPDIR}/symlink-intermediate-output"

mutated_support_project="${TEST_TMPDIR}/mutated-support-project"
cp -R "${project}" "${mutated_support_project}"
printf 'mutated symbols\n' >"${mutated_support_project}/CircuitC.kicad_sym"
expect_failure mutated_support "${runner}" \
  --request "${TEST_TMPDIR}/request.json" \
  --project-dir "${mutated_support_project}" \
  --fabrication-manifest "${TEST_TMPDIR}/fabrication-manifest.json" \
  --kicad-cli "${fake_kicad}" \
  --normalizer "${normalizer}" \
  --host-runner "${fake_host}" \
  --work-dir "${TEST_TMPDIR}/work" \
  --output-root "${TEST_TMPDIR}/mutated-support-output"

expect_failure stdout_over "${runner}" \
  --request "${TEST_TMPDIR}/request.json" \
  --project-dir "${project}" \
  --fabrication-manifest "${TEST_TMPDIR}/fabrication-manifest.json" \
  --kicad-cli "${fake_kicad}" \
  --normalizer "${normalizer}" \
  --host-runner "${fake_over}" \
  --work-dir "${TEST_TMPDIR}/work" \
  --output-root "${TEST_TMPDIR}/stdout-over-output"

cp "${fake_kicad_source}" "${TEST_TMPDIR}/hardlink-target"
ln "${TEST_TMPDIR}/hardlink-target" "${TEST_TMPDIR}/hardlink-kicad"
expect_failure hardlink "${runner}" \
  --request "${TEST_TMPDIR}/request.json" \
  --project-dir "${project}" \
  --fabrication-manifest "${TEST_TMPDIR}/fabrication-manifest.json" \
  --kicad-cli "${TEST_TMPDIR}/hardlink-kicad" \
  --normalizer "${normalizer}" \
  --host-runner "${fake_host}" \
  --work-dir "${TEST_TMPDIR}/work" \
  --output-root "${TEST_TMPDIR}/hardlink-output"
