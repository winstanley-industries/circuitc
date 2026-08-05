#!/usr/bin/env -S python3 -I
"""Execute one canonical CircuitC KiCad board-analysis request."""

from __future__ import annotations

import argparse
import ctypes
import errno
import hashlib
import json
import os
import pathlib
import re
import secrets
import selectors
import signal
import stat
import subprocess
import sys
import time

MAX_FILE_BYTES = 64 * 1024 * 1024
MAX_AGGREGATE_BYTES = 256 * 1024 * 1024
MAX_STDIO_BYTES = 1024 * 1024
IDENTITY_DOMAIN = b"CIRCUITC-BOARD-ANALYSIS-IDENTITY-V1\0"
EXPECTED_OUTPUTS = [
    {"role": "erc", "path": "erc.normalized.json"},
    {"role": "drc", "path": "drc.normalized.json"},
    {"role": "receipt", "path": "receipt.json"},
]
EXPECTED_POLICY = {
    "included_severities": ["error", "exclusion", "warning"],
    "erc_ignored_checks": [
        "footprint_filter",
        "four_way_junction",
        "simulation_model_issue",
        "single_global_label",
    ],
    "drc_ignored_checks": [
        "footprint_filters_mismatch",
        "footprint_type_mismatch",
        "missing_courtyard",
        "track_not_centered_on_via",
        "tuning_profile_track_geometries",
    ],
    "drc_library_warning": (
        "The current configuration does not include the footprint library 'CircuitC'"
    ),
}
EXPECTED_RESOURCES = {
    "timeout_ms": 120000,
    "stdout_bytes": 1048576,
    "stderr_bytes": 1048576,
    "file_bytes": 67108864,
    "aggregate_bytes": 268435456,
    "primary_rows": 10000,
    "diagnostics": 256,
}
UUID_V8_PATTERN = re.compile(
    r"[0-9a-f]{8}-[0-9a-f]{4}-8[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}\Z"
)


class AnalysisHostError(Exception):
    pass


def _checked_aggregate_add(current: int, addition: int) -> int:
    if current < 0 or addition < 0 or current > MAX_AGGREGATE_BYTES - addition:
        raise AnalysisHostError("analysis inputs and outputs exceed the aggregate limit")
    return current + addition


def _strict_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        if key in result:
            raise AnalysisHostError(f"duplicate request key {key!r}")
        result[key] = value
    return result


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _identity(metadata: os.stat_result) -> tuple[int, int, int, int, int, int, int]:
    return (
        metadata.st_dev,
        metadata.st_ino,
        metadata.st_size,
        metadata.st_mode,
        metadata.st_nlink,
        metadata.st_mtime_ns,
        metadata.st_ctime_ns,
    )


def _directory_flags() -> int:
    return (
        os.O_RDONLY
        | getattr(os, "O_DIRECTORY", 0)
        | getattr(os, "O_NOFOLLOW", 0)
        | getattr(os, "O_CLOEXEC", 0)
    )


def _directory_identity(metadata: os.stat_result) -> tuple[int, int, int, int]:
    return (metadata.st_dev, metadata.st_ino, metadata.st_uid, metadata.st_mode)


def _validate_directory(
    metadata: os.stat_result,
    path: pathlib.Path,
    *,
    protected_namespace: bool,
    private_terminal: bool,
) -> None:
    if not stat.S_ISDIR(metadata.st_mode):
        raise AnalysisHostError(f"anchored path component is not a directory: {path}")
    shared_write = metadata.st_mode & (stat.S_IWGRP | stat.S_IWOTH)
    if protected_namespace and metadata.st_uid not in {0, os.geteuid()}:
        raise AnalysisHostError(f"anchored directory has an untrusted owner: {path}")
    if (
        protected_namespace
        and shared_write
        and (not metadata.st_mode & stat.S_ISVTX or metadata.st_uid not in {0, os.geteuid()})
    ):
        raise AnalysisHostError(f"anchored directory permits unsafe shared writes: {path}")
    if private_terminal and (metadata.st_uid != os.geteuid() or shared_write):
        raise AnalysisHostError(
            f"private directory must be owned by the effective uid and not group/other writable: {path}"
        )


def _open_anchored_directory(path: pathlib.Path, *, require_private_owner: bool = False) -> int:
    absolute = path.absolute()
    descriptor = os.open(absolute.anchor, _directory_flags())
    try:
        _validate_directory(
            os.fstat(descriptor),
            pathlib.Path(absolute.anchor),
            protected_namespace=require_private_owner,
            private_terminal=False,
        )
        components = absolute.parts[1:]
        walked = pathlib.Path(absolute.anchor)
        for index, component in enumerate(components):
            next_descriptor = os.open(component, _directory_flags(), dir_fd=descriptor)
            os.close(descriptor)
            descriptor = next_descriptor
            walked /= component
            _validate_directory(
                os.fstat(descriptor),
                walked,
                protected_namespace=require_private_owner,
                private_terminal=require_private_owner and index == len(components) - 1,
            )
        if not components and require_private_owner:
            _validate_directory(
                os.fstat(descriptor),
                absolute,
                protected_namespace=True,
                private_terminal=True,
            )
        return descriptor
    except BaseException:
        os.close(descriptor)
        raise


def _read_bounded(
    path: pathlib.Path, maximum: int = MAX_FILE_BYTES
) -> tuple[bytes, tuple[int, ...]]:
    parent = _open_anchored_directory(path.absolute().parent)
    try:
        descriptor = os.open(
            path.name,
            os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_CLOEXEC", 0),
            dir_fd=parent,
        )
    finally:
        os.close(parent)
    try:
        before = os.fstat(descriptor)
        effective_maximum = min(MAX_FILE_BYTES, maximum)
        if (
            not stat.S_ISREG(before.st_mode)
            or before.st_nlink != 1
            or before.st_size > effective_maximum
        ):
            raise AnalysisHostError(f"input is not a bounded single-link regular file: {path}")
        chunks: list[bytes] = []
        total = 0
        while chunk := os.read(descriptor, 8192):
            total += len(chunk)
            if total > effective_maximum:
                raise AnalysisHostError(f"input exceeds the 64 MiB limit: {path}")
            chunks.append(chunk)
        after = os.fstat(descriptor)
        if _identity(after) != _identity(before) or total != before.st_size:
            raise AnalysisHostError(f"input changed while it was read: {path}")
        return b"".join(chunks), _identity(before)
    finally:
        os.close(descriptor)


def _verify_path(path: pathlib.Path, expected: tuple[int, ...], digest: str) -> None:
    data, identity = _read_bounded(path)
    if identity != expected or _sha256(data) != digest:
        raise AnalysisHostError(f"authenticated input changed during board analysis: {path}")


def _artifact(binding: object, label: str) -> dict[str, object]:
    if not isinstance(binding, dict) or list(binding) != ["path", "byte_length", "sha256"]:
        raise AnalysisHostError(f"request {label} binding has an unsupported shape")
    path = binding["path"]
    length = binding["byte_length"]
    digest = binding["sha256"]
    if (
        not isinstance(path, str)
        or not path
        or path.startswith("/")
        or "\\" in path
        or any(part in {"", ".", ".."} for part in path.split("/"))
        or type(length) is not int
        or not 0 <= length <= MAX_FILE_BYTES
        or not isinstance(digest, str)
        or len(digest) != 64
        or any(character not in "0123456789abcdef" for character in digest)
    ):
        raise AnalysisHostError(f"request {label} binding is not canonical")
    return binding


def _load_request(path: pathlib.Path) -> tuple[dict[str, object], bytes]:
    raw, _ = _read_bounded(path)
    if not raw.endswith(b"\n") or b"\r" in raw or b"\0" in raw:
        raise AnalysisHostError("analysis request must be canonical JSON plus one LF")
    try:
        request = json.loads(raw, object_pairs_hook=_strict_object)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise AnalysisHostError(f"analysis request is invalid JSON: {error}") from error
    if not isinstance(request, dict):
        raise AnalysisHostError("analysis request root must be an object")
    expected_keys = [
        "schema_name",
        "schema_version",
        "design_name",
        "analysis_path",
        "adapter",
        "expected_major",
        "expected_version",
        "analysis_identity_sha256",
        "assertions",
        "kicad_schematic",
        "kicad_pcb",
        "kicad_identity_map",
        "expected_sheets",
        "project_support",
        "fabrication_request",
        "fabrication_manifest",
        "policy",
        "resources",
        "outputs",
    ]
    if list(request) != expected_keys:
        raise AnalysisHostError("analysis request key order or set is unsupported")
    if (
        request["schema_name"] != "circuitc.board_analysis_request"
        or type(request["schema_version"]) is not int
        or request["schema_version"] != 1
        or request["adapter"] != "kicad"
        or type(request["expected_major"]) is not int
        or request["expected_major"] != 10
        or request["expected_version"] != "10.0.5"
        or not isinstance(request["design_name"], str)
        or re.fullmatch(r"[A-Za-z_][A-Za-z0-9_-]*", request["design_name"]) is None
        or not isinstance(request["analysis_path"], str)
        or not request["analysis_path"]
        or not isinstance(request["analysis_identity_sha256"], str)
    ):
        raise AnalysisHostError("analysis request identity or adapter is unsupported")
    for key in (
        "kicad_schematic",
        "kicad_pcb",
        "kicad_identity_map",
        "fabrication_request",
        "fabrication_manifest",
    ):
        _artifact(request[key], key)
    design_name = request["design_name"]
    if (
        request["kicad_schematic"]["path"] != f"{design_name}.kicad_sch"
        or request["kicad_pcb"]["path"] != f"{design_name}.kicad_pcb"
        or request["kicad_identity_map"]["path"] != f"{design_name}.kicad-map.json"
    ):
        raise AnalysisHostError("analysis request KiCad artifact paths are unsupported")
    expected_sheets = request["expected_sheets"]
    if (
        not isinstance(expected_sheets, list)
        or len(expected_sheets) != 1
        or not isinstance(expected_sheets[0], dict)
        or list(expected_sheets[0]) != ["path", "uuid_path"]
        or expected_sheets[0]["path"] != "/"
        or not isinstance(expected_sheets[0]["uuid_path"], str)
        or not expected_sheets[0]["uuid_path"].startswith("/")
        or UUID_V8_PATTERN.fullmatch(expected_sheets[0]["uuid_path"][1:]) is None
    ):
        raise AnalysisHostError("analysis request expected-sheet inventory is unsupported")
    project_support = request["project_support"]
    if (
        not isinstance(project_support, list)
        or not project_support
        or len(project_support) > 10_000
    ):
        raise AnalysisHostError("analysis request project-support inventory is unsupported")
    support_paths: list[str] = []
    for index, binding in enumerate(project_support):
        support_paths.append(_artifact(binding, f"project_support[{index}]")["path"])
    if support_paths != sorted(set(support_paths)):
        raise AnalysisHostError("analysis request project-support paths are not sorted and unique")
    all_project_paths = [
        request["kicad_schematic"]["path"],
        request["kicad_pcb"]["path"],
        request["kicad_identity_map"]["path"],
        *support_paths,
    ]
    path_parts = [tuple(path.split("/")) for path in all_project_paths]
    if len(set(all_project_paths)) != len(all_project_paths) or any(
        left != right and len(left) <= len(right) and right[: len(left)] == left
        for left in path_parts
        for right in path_parts
    ):
        raise AnalysisHostError("analysis request project inventory contains a path collision")
    assertions = request["assertions"]
    expected_capabilities = [
        "erc_clean",
        "drc_clean",
        "unconnected_clean",
        "schematic_parity_clean",
        "fabrication_inventory_complete",
    ]
    if not isinstance(assertions, list) or len(assertions) != 5:
        raise AnalysisHostError("analysis request must declare exactly five assertions")
    for assertion, capability in zip(assertions, expected_capabilities):
        if (
            not isinstance(assertion, dict)
            or list(assertion) != ["assertion_path", "capability"]
            or not isinstance(assertion["assertion_path"], str)
            or assertion["capability"] != capability
        ):
            raise AnalysisHostError("analysis assertion inventory is not canonical")
    policy = request["policy"]
    resources = request["resources"]
    if (
        not isinstance(policy, dict)
        or list(policy) != list(EXPECTED_POLICY)
        or policy != EXPECTED_POLICY
        or not isinstance(resources, dict)
        or list(resources) != list(EXPECTED_RESOURCES)
        or resources != EXPECTED_RESOURCES
        or any(type(resources[key]) is not int for key in EXPECTED_RESOURCES)
        or any(
            not isinstance(value, str)
            for key in ("included_severities", "erc_ignored_checks", "drc_ignored_checks")
            for value in policy[key]
        )
        or not isinstance(policy["drc_library_warning"], str)
    ):
        raise AnalysisHostError("analysis request policy or resources are unsupported")
    if (
        request["outputs"] != EXPECTED_OUTPUTS
        or not isinstance(request["outputs"], list)
        or any(
            not isinstance(output, dict) or list(output) != ["role", "path"]
            for output in request["outputs"]
        )
    ):
        raise AnalysisHostError("analysis request output inventory is unsupported")
    canonical = json.dumps(request, separators=(",", ":"), ensure_ascii=True).encode() + b"\n"
    if canonical != raw:
        raise AnalysisHostError("analysis request bytes are not canonical")
    preimage = {
        key: request[key]
        for key in expected_keys
        if key not in {"schema_name", "schema_version", "analysis_identity_sha256"}
    }
    identity = _sha256(
        IDENTITY_DOMAIN + json.dumps(preimage, separators=(",", ":"), ensure_ascii=True).encode()
    )
    if request["analysis_identity_sha256"] != identity:
        raise AnalysisHostError("analysis request identity digest is invalid")
    return request, raw


def _verify_binding(
    binding: dict[str, object], path: pathlib.Path, maximum: int
) -> tuple[bytes, tuple[int, ...]]:
    if binding["byte_length"] > min(MAX_FILE_BYTES, maximum):
        raise AnalysisHostError(f"request binding exceeds the remaining aggregate budget: {path}")
    data, identity = _read_bounded(path, maximum)
    if len(data) != binding["byte_length"] or _sha256(data) != binding["sha256"]:
        raise AnalysisHostError(f"request binding does not match {path}")
    return data, identity


def _kill_process_group(process: subprocess.Popen[bytes]) -> None:
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass


def _run_process(
    command: list[str],
    timeout_seconds: float,
    label: str,
    environment: dict[str, str],
) -> tuple[bytes, bytes]:
    process = subprocess.Popen(
        command,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=environment,
        start_new_session=True,
    )
    if process.stdout is None or process.stderr is None:
        _kill_process_group(process)
        process.wait()
        raise AnalysisHostError(f"{label} did not expose bounded process streams")
    streams = selectors.DefaultSelector()
    streams.register(process.stdout, selectors.EVENT_READ, "stdout")
    streams.register(process.stderr, selectors.EVENT_READ, "stderr")
    captured = {"stdout": bytearray(), "stderr": bytearray()}
    deadline = time.monotonic() + timeout_seconds
    try:
        while streams.get_map():
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                _kill_process_group(process)
                process.wait()
                raise AnalysisHostError(f"{label} exceeded the deadline")
            for key, _ in streams.select(min(remaining, 0.1)):
                chunk = os.read(key.fd, 8192)
                if not chunk:
                    streams.unregister(key.fileobj)
                    continue
                output = captured[key.data]
                output.extend(chunk)
                if len(output) > MAX_STDIO_BYTES:
                    _kill_process_group(process)
                    process.wait()
                    raise AnalysisHostError(f"{label} exceeded the bounded stdio budget")
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            _kill_process_group(process)
            process.wait()
            raise AnalysisHostError(f"{label} exceeded the deadline")
        returncode = process.wait(timeout=remaining)
    except subprocess.TimeoutExpired as error:
        _kill_process_group(process)
        process.wait()
        raise AnalysisHostError(f"{label} exceeded the deadline") from error
    finally:
        streams.close()
        process.stdout.close()
        process.stderr.close()
    stdout = bytes(captured["stdout"])
    stderr = bytes(captured["stderr"])
    if returncode != 0:
        detail = stderr.decode(errors="replace").strip()
        raise AnalysisHostError(f"{label} failed with exit {returncode}: {detail}")
    return stdout, stderr


def _open_or_create_anchored_directory(
    path: pathlib.Path, *, require_private_owner: bool = False
) -> int:
    absolute = path.absolute()
    descriptor = os.open(absolute.anchor, _directory_flags())
    try:
        _validate_directory(
            os.fstat(descriptor),
            pathlib.Path(absolute.anchor),
            protected_namespace=require_private_owner,
            private_terminal=False,
        )
        components = absolute.parts[1:]
        walked = pathlib.Path(absolute.anchor)
        for index, component in enumerate(components):
            try:
                next_descriptor = os.open(component, _directory_flags(), dir_fd=descriptor)
            except FileNotFoundError:
                os.mkdir(component, mode=0o700, dir_fd=descriptor)
                next_descriptor = os.open(component, _directory_flags(), dir_fd=descriptor)
            os.close(descriptor)
            descriptor = next_descriptor
            walked /= component
            _validate_directory(
                os.fstat(descriptor),
                walked,
                protected_namespace=require_private_owner,
                private_terminal=require_private_owner and index == len(components) - 1,
            )
        if not components and require_private_owner:
            _validate_directory(
                os.fstat(descriptor),
                absolute,
                protected_namespace=True,
                private_terminal=True,
            )
        return descriptor
    except BaseException:
        os.close(descriptor)
        raise


def _recheck_directory_path(
    path: pathlib.Path,
    held_descriptor: int,
    expected: tuple[int, int, int, int],
    *,
    require_private_owner: bool,
) -> None:
    current = os.fstat(held_descriptor)
    _validate_directory(
        current,
        path,
        protected_namespace=require_private_owner,
        private_terminal=require_private_owner,
    )
    if _directory_identity(current) != expected:
        raise AnalysisHostError(f"held directory identity changed: {path}")
    reopened = _open_anchored_directory(path, require_private_owner=require_private_owner)
    try:
        if _directory_identity(os.fstat(reopened)) != expected:
            raise AnalysisHostError(f"anchored directory path was replaced: {path}")
    finally:
        os.close(reopened)


def _recheck_private_transaction(
    transaction: pathlib.Path,
    work_descriptor: int,
    work_identity: tuple[int, int, int, int],
    transaction_descriptor: int,
    transaction_identity: tuple[int, int, int, int],
) -> None:
    work_dir = transaction.parent
    _recheck_directory_path(
        work_dir,
        work_descriptor,
        work_identity,
        require_private_owner=True,
    )
    current = os.fstat(transaction_descriptor)
    _validate_directory(current, transaction, protected_namespace=True, private_terminal=True)
    named = os.stat(transaction.name, dir_fd=work_descriptor, follow_symlinks=False)
    if (
        _directory_identity(current) != transaction_identity
        or _directory_identity(named) != transaction_identity
    ):
        raise AnalysisHostError("private transaction directory was replaced")


def _remove_directory_contents(descriptor: int) -> None:
    for name in os.listdir(descriptor):
        metadata = os.stat(name, dir_fd=descriptor, follow_symlinks=False)
        if stat.S_ISDIR(metadata.st_mode):
            child = os.open(name, _directory_flags(), dir_fd=descriptor)
            identity = _directory_identity(os.fstat(child))
            try:
                if identity != _directory_identity(metadata):
                    raise AnalysisHostError("transaction child directory was replaced")
                _remove_directory_contents(child)
                named = os.stat(name, dir_fd=descriptor, follow_symlinks=False)
                if _directory_identity(named) != identity:
                    raise AnalysisHostError("transaction child directory was replaced")
                os.rmdir(name, dir_fd=descriptor)
            finally:
                os.close(child)
        else:
            os.unlink(name, dir_fd=descriptor)


def _create_private_transaction(
    work_dir: pathlib.Path,
) -> tuple[
    pathlib.Path,
    bool,
    int,
    tuple[int, int, int, int],
    int,
    tuple[int, int, int, int],
]:
    try:
        work_fd = _open_anchored_directory(work_dir, require_private_owner=True)
        created_work_dir = False
    except FileNotFoundError:
        work_fd = _open_or_create_anchored_directory(work_dir, require_private_owner=True)
        created_work_dir = True
    work_identity = _directory_identity(os.fstat(work_fd))
    name = f"analysis-{secrets.token_hex(12)}"
    try:
        os.mkdir(name, mode=0o700, dir_fd=work_fd)
        transaction_fd = os.open(name, _directory_flags(), dir_fd=work_fd)
    except BaseException:
        os.close(work_fd)
        raise
    transaction = work_dir.absolute() / name
    transaction_identity = _directory_identity(os.fstat(transaction_fd))
    _recheck_private_transaction(
        transaction,
        work_fd,
        work_identity,
        transaction_fd,
        transaction_identity,
    )
    return (
        transaction,
        created_work_dir,
        work_fd,
        work_identity,
        transaction_fd,
        transaction_identity,
    )


def _remove_empty_anchored_directory(
    path: pathlib.Path,
    held_descriptor: int,
    expected: tuple[int, int, int, int],
) -> None:
    parent = _open_anchored_directory(path.absolute().parent)
    try:
        current = os.fstat(held_descriptor)
        named = os.stat(path.name, dir_fd=parent, follow_symlinks=False)
        if _directory_identity(current) != expected or _directory_identity(named) != expected:
            raise AnalysisHostError("created work directory was replaced")
        os.rmdir(path.name, dir_fd=parent)
    finally:
        os.close(parent)


def _write_private_file(path: pathlib.Path, data: bytes, mode: int) -> None:
    parent_fd = _open_anchored_directory(path.parent)
    try:
        descriptor = os.open(
            path.name,
            os.O_WRONLY
            | os.O_CREAT
            | os.O_EXCL
            | getattr(os, "O_NOFOLLOW", 0)
            | getattr(os, "O_CLOEXEC", 0),
            mode,
            dir_fd=parent_fd,
        )
    finally:
        os.close(parent_fd)
    try:
        view = memoryview(data)
        while view:
            view = view[os.write(descriptor, view) :]
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def _private_environment(home: pathlib.Path, temporary: pathlib.Path) -> dict[str, str]:
    return {
        "HOME": str(home),
        "KICAD_CONFIG_HOME": str(home / "kicad"),
        "LC_ALL": "C",
        "PATH": "/usr/bin:/bin",
        "PYTHONDONTWRITEBYTECODE": "1",
        "PYTHONNOUSERSITE": "1",
        "TMPDIR": str(temporary),
    }


def _is_macho(data: bytes) -> bool:
    return data[:4] in {
        b"\xca\xfe\xba\xbe",
        b"\xbe\xba\xfe\xca",
        b"\xfe\xed\xfa\xce",
        b"\xce\xfa\xed\xfe",
        b"\xfe\xed\xfa\xcf",
        b"\xcf\xfa\xed\xfe",
    }


def _stage_executable(source: pathlib.Path, data: bytes, transaction: pathlib.Path) -> pathlib.Path:
    transaction.mkdir(mode=0o700)
    if not _is_macho(data):
        staged = transaction / "kicad-cli"
        _write_private_file(staged, data, 0o500)
        return staged
    source = source.absolute()
    if source.parent.name != "MacOS" or source.parent.parent.name != "Contents":
        raise AnalysisHostError("Mach-O KiCad executable is not inside a canonical app bundle")
    source_contents = source.parent.parent
    staged_contents = transaction / "KiCad.app" / "Contents"
    staged_macos = staged_contents / "MacOS"
    staged_macos.mkdir(parents=True, mode=0o700)
    staged = staged_macos / source.name
    _write_private_file(staged, data, 0o500)
    for name in ("Frameworks", "PlugIns", "Resources", "SharedSupport"):
        resource = source_contents / name
        if not resource.is_dir():
            raise AnalysisHostError(f"KiCad bundle resource directory is missing: {name}")
        os.symlink(resource, staged_contents / name, target_is_directory=True)
    return staged


def _rename_noreplace(parent_fd: int, source: str, destination: str) -> None:
    libc = ctypes.CDLL(None, use_errno=True)
    if hasattr(libc, "renameatx_np"):
        result = libc.renameatx_np(
            parent_fd,
            source.encode(),
            parent_fd,
            destination.encode(),
            0x00000004,
        )
    elif hasattr(libc, "renameat2"):
        result = libc.renameat2(
            parent_fd,
            source.encode(),
            parent_fd,
            destination.encode(),
            0x00000001,
        )
    else:
        raise AnalysisHostError("host has no atomic no-replace publication primitive")
    if result != 0:
        error_number = ctypes.get_errno()
        if error_number == errno.EEXIST:
            raise AnalysisHostError("analysis output already exists")
        raise AnalysisHostError(f"analysis publication failed: {os.strerror(error_number)}")


def _publish(output_root: pathlib.Path, outputs: dict[str, bytes]) -> None:
    parent = output_root.parent.absolute()
    parent_fd = _open_anchored_directory(parent, require_private_owner=True)
    parent_identity = _directory_identity(os.fstat(parent_fd))
    temporary = f".{output_root.name}.circuitc-{secrets.token_hex(12)}"
    published = False
    root_fd: int | None = None
    root_identity: tuple[int, int, int, int] | None = None
    try:
        os.mkdir(temporary, mode=0o700, dir_fd=parent_fd)
        root_fd = os.open(
            temporary,
            os.O_RDONLY
            | getattr(os, "O_DIRECTORY", 0)
            | getattr(os, "O_NOFOLLOW", 0)
            | getattr(os, "O_CLOEXEC", 0),
            dir_fd=parent_fd,
        )
        root_identity = _directory_identity(os.fstat(root_fd))
        try:
            for name, data in sorted(outputs.items()):
                descriptor = os.open(
                    name,
                    os.O_WRONLY
                    | os.O_CREAT
                    | os.O_EXCL
                    | getattr(os, "O_NOFOLLOW", 0)
                    | getattr(os, "O_CLOEXEC", 0),
                    0o600,
                    dir_fd=root_fd,
                )
                try:
                    view = memoryview(data)
                    while view:
                        view = view[os.write(descriptor, view) :]
                    os.fsync(descriptor)
                finally:
                    os.close(descriptor)
            os.fsync(root_fd)
            _recheck_directory_path(parent, parent_fd, parent_identity, require_private_owner=True)
            named_root = os.stat(temporary, dir_fd=parent_fd, follow_symlinks=False)
            if _directory_identity(named_root) != root_identity:
                raise AnalysisHostError("analysis publication directory was replaced")
        except BaseException:
            raise
        _rename_noreplace(parent_fd, temporary, output_root.name)
        published = True
        named_root = os.stat(output_root.name, dir_fd=parent_fd, follow_symlinks=False)
        if _directory_identity(named_root) != root_identity:
            raise AnalysisHostError("published analysis directory was replaced")
        os.fsync(parent_fd)
        _recheck_directory_path(parent, parent_fd, parent_identity, require_private_owner=True)
    finally:
        if not published:
            try:
                if root_fd is not None:
                    _remove_directory_contents(root_fd)
                    named_root = os.stat(temporary, dir_fd=parent_fd, follow_symlinks=False)
                    if root_identity is None or _directory_identity(named_root) != root_identity:
                        raise AnalysisHostError("analysis publication directory was replaced")
                    os.rmdir(temporary, dir_fd=parent_fd)
            except OSError:
                pass
        if root_fd is not None:
            os.close(root_fd)
        os.close(parent_fd)


def run(args: argparse.Namespace) -> None:
    request, request_bytes = _load_request(args.request)
    project = args.project_dir.absolute()
    schematic_path = project / pathlib.PurePosixPath(request["kicad_schematic"]["path"]).name
    pcb_path = project / pathlib.PurePosixPath(request["kicad_pcb"]["path"]).name
    identity_map_path = project / pathlib.PurePosixPath(request["kicad_identity_map"]["path"]).name
    input_aggregate = _checked_aggregate_add(0, len(request_bytes))
    schematic, schematic_identity = _verify_binding(
        request["kicad_schematic"], schematic_path, MAX_AGGREGATE_BYTES - input_aggregate
    )
    input_aggregate = _checked_aggregate_add(input_aggregate, len(schematic))
    pcb, pcb_identity = _verify_binding(
        request["kicad_pcb"], pcb_path, MAX_AGGREGATE_BYTES - input_aggregate
    )
    input_aggregate = _checked_aggregate_add(input_aggregate, len(pcb))
    identity_map, identity_map_identity = _verify_binding(
        request["kicad_identity_map"],
        identity_map_path,
        MAX_AGGREGATE_BYTES - input_aggregate,
    )
    input_aggregate = _checked_aggregate_add(input_aggregate, len(identity_map))
    support_inputs: list[tuple[dict[str, object], pathlib.Path, bytes, tuple[int, ...]]] = []
    for binding in request["project_support"]:
        support_path = project.joinpath(*pathlib.PurePosixPath(binding["path"]).parts)
        support_data, support_identity = _verify_binding(
            binding, support_path, MAX_AGGREGATE_BYTES - input_aggregate
        )
        input_aggregate = _checked_aggregate_add(input_aggregate, len(support_data))
        support_inputs.append((binding, support_path, support_data, support_identity))
    fabrication_manifest, fabrication_identity = _verify_binding(
        request["fabrication_manifest"],
        args.fabrication_manifest,
        MAX_AGGREGATE_BYTES - input_aggregate,
    )
    input_aggregate = _checked_aggregate_add(input_aggregate, len(fabrication_manifest))
    executable, executable_identity = _read_bounded(
        args.kicad_cli, MAX_AGGREGATE_BYTES - input_aggregate
    )
    input_aggregate = _checked_aggregate_add(input_aggregate, len(executable))
    normalizer, normalizer_identity = _read_bounded(
        args.normalizer, MAX_AGGREGATE_BYTES - input_aggregate
    )
    input_aggregate = _checked_aggregate_add(input_aggregate, len(normalizer))
    host_runner, host_runner_identity = _read_bounded(
        args.host_runner, MAX_AGGREGATE_BYTES - input_aggregate
    )
    input_aggregate = _checked_aggregate_add(input_aggregate, len(host_runner))

    (
        transaction,
        created_work_dir,
        work_fd,
        work_identity,
        transaction_fd,
        transaction_identity,
    ) = _create_private_transaction(args.work_dir)
    outputs_to_publish = None
    try:
        snapshot_project = transaction / "project"
        snapshot_tools = transaction / "tools"
        home = transaction / "home"
        temporary = transaction / "temp"
        for directory in (snapshot_project, snapshot_tools, home, home / "kicad", temporary):
            directory.mkdir(mode=0o700)
        snapshot_schematic = snapshot_project / schematic_path.name
        snapshot_pcb = snapshot_project / pcb_path.name
        snapshot_identity_map = snapshot_project / identity_map_path.name
        snapshot_normalizer = snapshot_tools / "normalize_drc.py"
        snapshot_host_runner = snapshot_tools / "run_host_validation.py"
        for path, data, mode in [
            (snapshot_schematic, schematic, 0o400),
            (snapshot_pcb, pcb, 0o400),
            (snapshot_identity_map, identity_map, 0o400),
            (snapshot_normalizer, normalizer, 0o400),
            (snapshot_host_runner, host_runner, 0o400),
        ]:
            _write_private_file(path, data, mode)
        snapshot_support: list[pathlib.Path] = []
        for binding, _, data, _ in support_inputs:
            staged = snapshot_project.joinpath(*pathlib.PurePosixPath(binding["path"]).parts)
            staged.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
            _write_private_file(staged, data, 0o400)
            snapshot_support.append(staged)
        snapshot_kicad = _stage_executable(args.kicad_cli, executable, transaction / "kicad")
        environment = _private_environment(home, temporary)
        _verify_path(args.kicad_cli, executable_identity, _sha256(executable))
        _recheck_private_transaction(
            transaction,
            work_fd,
            work_identity,
            transaction_fd,
            transaction_identity,
        )
        version_stdout, _ = _run_process(
            [str(snapshot_kicad), "--version"], 10, "KiCad version probe", environment
        )
        _verify_path(args.kicad_cli, executable_identity, _sha256(executable))
        if version_stdout.decode(errors="replace").strip() != "10.0.5":
            raise AnalysisHostError("board analysis requires exact KiCad 10.0.5")

        project_artifacts = [
            snapshot_schematic,
            snapshot_pcb,
            snapshot_identity_map,
            *snapshot_support,
        ]
        for kind, source_path, ignored in [
            ("erc", snapshot_schematic, EXPECTED_POLICY["erc_ignored_checks"]),
            ("drc", snapshot_pcb, EXPECTED_POLICY["drc_ignored_checks"]),
        ]:
            _recheck_private_transaction(
                transaction,
                work_fd,
                work_identity,
                transaction_fd,
                transaction_identity,
            )
            _verify_path(args.kicad_cli, executable_identity, _sha256(executable))
            command = [
                sys.executable,
                "-I",
                str(snapshot_host_runner),
                "--kicad-cli",
                str(snapshot_kicad),
                "--normalizer",
                str(snapshot_normalizer),
                "--kind",
                kind,
                "--source-artifact",
                str(source_path),
                "--identity-map",
                str(snapshot_identity_map),
                "--raw-output",
                str(transaction / f"{kind}.raw.json"),
                "--normalized-output",
                str(transaction / f"{kind}.normalized.json"),
                "--work-dir",
                str(transaction / "host-work"),
                "--expected-major",
                "10",
                "--retain-findings",
            ]
            for artifact in project_artifacts:
                command.extend(("--project-artifact", str(artifact)))
            for check in ignored:
                command.extend(("--allow-ignored-check", check))
            _run_process(command, 130, f"{kind} host analysis", environment)
            _recheck_private_transaction(
                transaction,
                work_fd,
                work_identity,
                transaction_fd,
                transaction_identity,
            )
            _verify_path(args.kicad_cli, executable_identity, _sha256(executable))

        erc, _ = _read_bounded(
            transaction / "erc.normalized.json", MAX_AGGREGATE_BYTES - input_aggregate
        )
        output_aggregate = _checked_aggregate_add(input_aggregate, len(erc))
        drc, _ = _read_bounded(
            transaction / "drc.normalized.json", MAX_AGGREGATE_BYTES - output_aggregate
        )
        output_aggregate = _checked_aggregate_add(output_aggregate, len(drc))
        for path, identity, digest in [
            (schematic_path, schematic_identity, request["kicad_schematic"]["sha256"]),
            (pcb_path, pcb_identity, request["kicad_pcb"]["sha256"]),
            (identity_map_path, identity_map_identity, request["kicad_identity_map"]["sha256"]),
            (
                args.fabrication_manifest,
                fabrication_identity,
                request["fabrication_manifest"]["sha256"],
            ),
            (args.kicad_cli, executable_identity, _sha256(executable)),
            (args.normalizer, normalizer_identity, _sha256(normalizer)),
            (args.host_runner, host_runner_identity, _sha256(host_runner)),
            *[(path, identity, binding["sha256"]) for binding, path, _, identity in support_inputs],
        ]:
            _verify_path(path, identity, digest)
        receipt = {
            "schema_name": "circuitc.board_analysis_receipt",
            "schema_version": 1,
            "request_sha256": _sha256(request_bytes),
            "schematic_sha256": request["kicad_schematic"]["sha256"],
            "pcb_sha256": request["kicad_pcb"]["sha256"],
            "identity_map_sha256": request["kicad_identity_map"]["sha256"],
            "executable_sha256": _sha256(executable),
            "normalizer_sha256": _sha256(normalizer),
            "host_runner_sha256": _sha256(host_runner),
            "erc_sha256": _sha256(erc),
            "drc_sha256": _sha256(drc),
        }
        receipt_bytes = json.dumps(receipt, separators=(",", ":")).encode() + b"\n"
        _checked_aggregate_add(output_aggregate, len(receipt_bytes))
        outputs_to_publish = {
            "erc.normalized.json": erc,
            "drc.normalized.json": drc,
            "receipt.json": receipt_bytes,
        }
    finally:
        try:
            _recheck_private_transaction(
                transaction,
                work_fd,
                work_identity,
                transaction_fd,
                transaction_identity,
            )
            _remove_directory_contents(transaction_fd)
            named_transaction = os.stat(transaction.name, dir_fd=work_fd, follow_symlinks=False)
            if _directory_identity(named_transaction) != transaction_identity:
                raise AnalysisHostError("private transaction directory was replaced")
            os.rmdir(transaction.name, dir_fd=work_fd)
            if created_work_dir:
                _remove_empty_anchored_directory(args.work_dir, work_fd, work_identity)
        finally:
            os.close(transaction_fd)
            os.close(work_fd)
    if outputs_to_publish is None:
        raise AnalysisHostError("board analysis completed without a complete output set")
    _publish(args.output_root, outputs_to_publish)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--request", required=True, type=pathlib.Path)
    parser.add_argument("--project-dir", required=True, type=pathlib.Path)
    parser.add_argument("--fabrication-manifest", required=True, type=pathlib.Path)
    parser.add_argument("--kicad-cli", required=True, type=pathlib.Path)
    parser.add_argument("--normalizer", required=True, type=pathlib.Path)
    parser.add_argument("--host-runner", required=True, type=pathlib.Path)
    parser.add_argument("--work-dir", required=True, type=pathlib.Path)
    parser.add_argument("--output-root", required=True, type=pathlib.Path)
    args = parser.parse_args()
    try:
        run(args)
    except (AnalysisHostError, OSError, subprocess.TimeoutExpired) as error:
        print(f"CircuitC board analysis failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
