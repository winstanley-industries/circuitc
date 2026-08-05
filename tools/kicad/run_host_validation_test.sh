#!/bin/bash
set -euo pipefail

runner="$1"

python3 -I - "${runner}" "${TEST_TMPDIR}" <<'PY'
import importlib.util
import os
import pathlib
import stat
import subprocess
import sys
import tempfile
import types

spec = importlib.util.spec_from_file_location("host_runner", sys.argv[1])
runner = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(runner)

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
except runner.HostValidationError:
    pass
else:
    raise AssertionError("accepted a protected ancestor owned by an untrusted uid")

with tempfile.TemporaryDirectory(dir=sys.argv[2]) as directory_name:
    directory = pathlib.Path(directory_name)
    exact = directory / "exact.raw.json"
    with exact.open("wb") as output:
        output.truncate(runner.MAX_SOURCE_BYTES)
    data, _ = runner._read_source_handle(exact, runner.MAX_AGGREGATE_BYTES)
    assert len(data) == runner.MAX_SOURCE_BYTES
    del data

    with exact.open("r+b") as output:
        output.truncate(runner.MAX_SOURCE_BYTES + 1)
    try:
        runner._read_source_handle(exact, runner.MAX_AGGREGATE_BYTES)
    except runner.HostValidationError:
        pass
    else:
        raise AssertionError("accepted a raw report one byte over the file limit")

    monitored = directory / "monitored.raw.json"
    writer = (
        "import pathlib,sys,time; "
        f"pathlib.Path({str(monitored)!r}).open('wb').truncate({runner.MAX_SOURCE_BYTES + 1}); "
        "time.sleep(1)"
    )
    try:
        runner._run(
            [sys.executable, "-I", "-c", writer],
            {"LC_ALL": "C", "PATH": "/usr/bin:/bin"},
            "oversized raw",
            [monitored],
        )
    except runner.HostValidationError:
        pass
    else:
        raise AssertionError("host raw-output monitor accepted an oversized file")

    original_timeout = runner.TIMEOUT_SECONDS
    runner.TIMEOUT_SECONDS = 0.05
    try:
        try:
            runner._run(
                [sys.executable, "-I", "-c", "import time; time.sleep(1)"],
                {"LC_ALL": "C", "PATH": "/usr/bin:/bin"},
                "timeout",
            )
        except runner.HostValidationError:
            pass
        else:
            raise AssertionError("host runner accepted a process beyond its deadline")
    finally:
        runner.TIMEOUT_SECONDS = original_timeout

    source = directory / "caller-kicad"
    source.write_bytes(b"#!/bin/sh\nprintf 'authenticated\\n'\n")
    source.chmod(0o700)
    transaction = directory / "transaction"
    staged = runner._stage_executable(source, source.read_bytes(), transaction)
    source.write_bytes(b"#!/bin/sh\nprintf 'attacker\\n'\n")
    completed = subprocess.run(
        [str(staged)],
        check=True,
        capture_output=True,
        text=True,
        env={"LC_ALL": "C", "PATH": "/usr/bin:/bin"},
    )
    assert completed.stdout == "authenticated\n"
PY
