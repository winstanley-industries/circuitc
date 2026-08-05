#!/usr/bin/env python3
"""Run KiCad and normalize its report against one immutable project snapshot."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import pathlib
import secrets
import selectors
import signal
import stat
import subprocess
import sys
import time

MAX_SOURCE_BYTES = 64 * 1024 * 1024
MAX_AGGREGATE_BYTES = 256 * 1024 * 1024
MAX_STDIO_BYTES = 1024 * 1024
TIMEOUT_SECONDS = 120


class HostValidationError(Exception):
    pass


def _checked_aggregate_add(current: int, addition: int) -> int:
    if current < 0 or addition < 0 or current > MAX_AGGREGATE_BYTES - addition:
        raise HostValidationError("host inputs and outputs exceed the aggregate limit")
    return current + addition


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
        raise HostValidationError(f"anchored path component is not a directory: {path}")
    shared_write = metadata.st_mode & (stat.S_IWGRP | stat.S_IWOTH)
    if protected_namespace and metadata.st_uid not in {0, os.geteuid()}:
        raise HostValidationError(f"anchored directory has an untrusted owner: {path}")
    if (
        protected_namespace
        and shared_write
        and (not metadata.st_mode & stat.S_ISVTX or metadata.st_uid not in {0, os.geteuid()})
    ):
        raise HostValidationError(f"anchored directory permits unsafe shared writes: {path}")
    if private_terminal and (metadata.st_uid != os.geteuid() or shared_write):
        raise HostValidationError(
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
        raise HostValidationError(f"held directory identity changed: {path}")
    reopened = _open_anchored_directory(path, require_private_owner=require_private_owner)
    try:
        if _directory_identity(os.fstat(reopened)) != expected:
            raise HostValidationError(f"anchored directory path was replaced: {path}")
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
        raise HostValidationError("private transaction directory was replaced")


def _remove_directory_contents(descriptor: int) -> None:
    for name in os.listdir(descriptor):
        metadata = os.stat(name, dir_fd=descriptor, follow_symlinks=False)
        if stat.S_ISDIR(metadata.st_mode):
            child = os.open(name, _directory_flags(), dir_fd=descriptor)
            identity = _directory_identity(os.fstat(child))
            try:
                if identity != _directory_identity(metadata):
                    raise HostValidationError("transaction child directory was replaced")
                _remove_directory_contents(child)
                named = os.stat(name, dir_fd=descriptor, follow_symlinks=False)
                if _directory_identity(named) != identity:
                    raise HostValidationError("transaction child directory was replaced")
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
        work = _open_anchored_directory(work_dir, require_private_owner=True)
        created_work_dir = False
    except FileNotFoundError:
        work = _open_or_create_anchored_directory(work_dir, require_private_owner=True)
        created_work_dir = True
    work_identity = _directory_identity(os.fstat(work))
    name = f"circuitc-kicad-{secrets.token_hex(12)}"
    try:
        os.mkdir(name, mode=0o700, dir_fd=work)
        transaction_fd = os.open(name, _directory_flags(), dir_fd=work)
    except BaseException:
        os.close(work)
        raise
    transaction = work_dir.absolute() / name
    transaction_identity = _directory_identity(os.fstat(transaction_fd))
    _recheck_private_transaction(
        transaction,
        work,
        work_identity,
        transaction_fd,
        transaction_identity,
    )
    return (
        transaction,
        created_work_dir,
        work,
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
            raise HostValidationError("created work directory was replaced")
        os.rmdir(path.name, dir_fd=parent)
    finally:
        os.close(parent)


def _file_identity(metadata: os.stat_result) -> tuple[int, ...]:
    return (
        metadata.st_dev,
        metadata.st_ino,
        metadata.st_size,
        metadata.st_mode,
        metadata.st_nlink,
        metadata.st_mtime_ns,
        metadata.st_ctime_ns,
    )


def _read_source_handle(path: pathlib.Path, remaining: int) -> tuple[bytes, str]:
    nofollow = getattr(os, "O_NOFOLLOW", 0)
    if nofollow == 0:
        raise HostValidationError("host cannot open the KiCad source without following links")
    parent = _open_anchored_directory(path.absolute().parent)
    try:
        descriptor = os.open(
            path.name,
            os.O_RDONLY | nofollow | getattr(os, "O_CLOEXEC", 0),
            dir_fd=parent,
        )
    finally:
        os.close(parent)
    try:
        before = os.fstat(descriptor)
        maximum = min(MAX_SOURCE_BYTES, remaining)
        if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1 or before.st_size > maximum:
            raise HostValidationError("KiCad source is not a bounded regular file")
        chunks: list[bytes] = []
        total = 0
        while chunk := os.read(descriptor, 8192):
            total += len(chunk)
            if total > maximum:
                raise HostValidationError("KiCad source exceeds the byte limit")
            chunks.append(chunk)
        after = os.fstat(descriptor)
        if _file_identity(after) != _file_identity(before) or total != before.st_size:
            raise HostValidationError("KiCad source changed while its snapshot was read")
        data = b"".join(chunks)
        return data, hashlib.sha256(data).hexdigest()
    finally:
        os.close(descriptor)


def _write_private_file(path: pathlib.Path, data: bytes, mode: int) -> None:
    parent = _open_anchored_directory(path.absolute().parent)
    try:
        descriptor = os.open(
            path.name,
            os.O_WRONLY
            | os.O_CREAT
            | os.O_EXCL
            | getattr(os, "O_NOFOLLOW", 0)
            | getattr(os, "O_CLOEXEC", 0),
            mode,
            dir_fd=parent,
        )
    finally:
        os.close(parent)
    try:
        view = memoryview(data)
        while view:
            view = view[os.write(descriptor, view) :]
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


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
        raise HostValidationError("Mach-O KiCad executable is not inside a canonical app bundle")
    source_contents = source.parent.parent
    staged_contents = transaction / "KiCad.app" / "Contents"
    staged_macos = staged_contents / "MacOS"
    staged_macos.mkdir(parents=True, mode=0o700)
    staged = staged_macos / source.name
    _write_private_file(staged, data, 0o500)
    for name in ("Frameworks", "PlugIns", "Resources", "SharedSupport"):
        resource = source_contents / name
        if not resource.is_dir():
            raise HostValidationError(f"KiCad bundle resource directory is missing: {name}")
        os.symlink(resource, staged_contents / name, target_is_directory=True)
    return staged


def _hash_open_file(descriptor: int) -> str:
    os.lseek(descriptor, 0, os.SEEK_SET)
    digest = hashlib.sha256()
    while chunk := os.read(descriptor, 8192):
        digest.update(chunk)
    return digest.hexdigest()


def _verify_staged_source(
    descriptor: int,
    identity: tuple[int, ...],
    digest: str,
    path: pathlib.Path,
) -> None:
    metadata = os.fstat(descriptor)
    path_metadata = path.lstat()
    if (
        _file_identity(metadata) != identity
        or _file_identity(path_metadata) != identity
        or not stat.S_ISREG(path_metadata.st_mode)
        or _hash_open_file(descriptor) != digest
    ):
        raise HostValidationError("staged KiCad source changed during host validation")


def _check_output_bounds(paths: list[pathlib.Path]) -> None:
    for path in paths:
        try:
            size = path.stat().st_size
        except FileNotFoundError:
            continue
        if size > MAX_SOURCE_BYTES:
            raise HostValidationError(f"host output exceeds the 64 MiB limit: {path.name}")


def _kill_process_group(process: subprocess.Popen[bytes]) -> None:
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass


def _run(
    command: list[str],
    environment: dict[str, str],
    label: str,
    bounded_outputs: list[pathlib.Path] | None = None,
) -> None:
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
        raise HostValidationError(f"{label} did not expose bounded process streams")
    streams = selectors.DefaultSelector()
    streams.register(process.stdout, selectors.EVENT_READ, "stdout")
    streams.register(process.stderr, selectors.EVENT_READ, "stderr")
    captured = {"stdout": bytearray(), "stderr": bytearray()}
    deadline = time.monotonic() + TIMEOUT_SECONDS
    try:
        while streams.get_map():
            _check_output_bounds(bounded_outputs or [])
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                _kill_process_group(process)
                process.wait()
                raise HostValidationError(f"{label} exceeded the {TIMEOUT_SECONDS}s deadline")
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
                    raise HostValidationError(f"{label} exceeded the bounded stdout/stderr budget")
        remaining = deadline - time.monotonic()
        _check_output_bounds(bounded_outputs or [])
        if remaining <= 0:
            _kill_process_group(process)
            process.wait()
            raise HostValidationError(f"{label} exceeded the {TIMEOUT_SECONDS}s deadline")
        returncode = process.wait(timeout=remaining)
    except subprocess.TimeoutExpired as error:
        try:
            _kill_process_group(process)
        except ProcessLookupError:
            pass
        process.wait()
        raise HostValidationError(f"{label} exceeded the {TIMEOUT_SECONDS}s deadline") from error
    except BaseException:
        _kill_process_group(process)
        process.wait()
        raise
    finally:
        streams.close()
        process.stdout.close()
        process.stderr.close()
    if returncode != 0:
        stderr = bytes(captured["stderr"]).decode("utf-8", errors="replace").strip()
        raise HostValidationError(f"{label} failed: {stderr}")


def _publish_pair(
    raw_path: pathlib.Path,
    raw_data: bytes,
    normalized_path: pathlib.Path,
    normalized_data: bytes,
) -> None:
    raw_path = raw_path.absolute()
    normalized_path = normalized_path.absolute()
    if raw_path.parent != normalized_path.parent or raw_path.name == normalized_path.name:
        raise HostValidationError("host outputs must be distinct files in one existing directory")
    parent_path = raw_path.parent
    parent = _open_anchored_directory(parent_path, require_private_owner=True)
    parent_identity = _directory_identity(os.fstat(parent))
    token = secrets.token_hex(12)
    raw_temporary = f".{raw_path.name}.{token}.tmp"
    normalized_temporary = f".{normalized_path.name}.{token}.tmp"
    published_raw = False
    published_normalized = False
    try:
        for name, data in (
            (raw_temporary, raw_data),
            (normalized_temporary, normalized_data),
        ):
            descriptor = os.open(
                name,
                os.O_WRONLY
                | os.O_CREAT
                | os.O_EXCL
                | getattr(os, "O_NOFOLLOW", 0)
                | getattr(os, "O_CLOEXEC", 0),
                0o600,
                dir_fd=parent,
            )
            try:
                view = memoryview(data)
                while view:
                    view = view[os.write(descriptor, view) :]
                os.fsync(descriptor)
            finally:
                os.close(descriptor)
        _recheck_directory_path(parent_path, parent, parent_identity, require_private_owner=True)
        os.link(
            raw_temporary,
            raw_path.name,
            src_dir_fd=parent,
            dst_dir_fd=parent,
            follow_symlinks=False,
        )
        published_raw = True
        os.link(
            normalized_temporary,
            normalized_path.name,
            src_dir_fd=parent,
            dst_dir_fd=parent,
            follow_symlinks=False,
        )
        published_normalized = True
        os.fsync(parent)
        _recheck_directory_path(parent_path, parent, parent_identity, require_private_owner=True)
    except FileExistsError as error:
        raise HostValidationError("host validation output already exists") from error
    finally:
        if published_raw and not published_normalized:
            try:
                os.unlink(raw_path.name, dir_fd=parent)
            except FileNotFoundError:
                pass
        for name in (raw_temporary, normalized_temporary):
            try:
                os.unlink(name, dir_fd=parent)
            except FileNotFoundError:
                pass
        os.close(parent)


def run(args: argparse.Namespace) -> None:
    source = args.source_artifact.absolute()
    identity_map = args.identity_map.absolute()
    project_root = source.parent
    project_artifacts = [path.absolute() for path in args.project_artifact]
    if not project_artifacts:
        project_artifacts = [source, identity_map]
    if source not in project_artifacts or identity_map not in project_artifacts:
        raise HostValidationError("source and identity map must be explicit project artifacts")
    relative_paths: dict[pathlib.Path, pathlib.PurePosixPath] = {}
    for artifact in project_artifacts:
        try:
            relative = artifact.relative_to(project_root)
        except ValueError as error:
            raise HostValidationError(
                "project artifact escapes the explicit project root"
            ) from error
        portable = pathlib.PurePosixPath(relative.as_posix())
        if (
            portable.is_absolute()
            or not portable.parts
            or any(part in {"", ".", ".."} for part in portable.parts)
        ):
            raise HostValidationError("project artifact path is not canonical")
        if artifact in relative_paths:
            raise HostValidationError("project artifact inventory contains a duplicate path")
        relative_paths[artifact] = portable
    path_parts = [relative.parts for relative in relative_paths.values()]
    if any(
        left != right and len(left) <= len(right) and right[: len(left)] == left
        for left in path_parts
        for right in path_parts
    ):
        raise HostValidationError("project artifact inventory contains a file/directory collision")

    aggregate = 0
    snapshots: dict[pathlib.Path, tuple[bytes, str]] = {}
    for artifact in project_artifacts:
        data, digest = _read_source_handle(artifact, MAX_AGGREGATE_BYTES - aggregate)
        aggregate = _checked_aggregate_add(aggregate, len(data))
        snapshots[artifact] = (data, digest)
    _, source_digest = snapshots[source]
    normalizer_data, normalizer_digest = _read_source_handle(
        args.normalizer, MAX_AGGREGATE_BYTES - aggregate
    )
    aggregate = _checked_aggregate_add(aggregate, len(normalizer_data))
    executable_data, executable_digest = _read_source_handle(
        args.kicad_cli, MAX_AGGREGATE_BYTES - aggregate
    )
    aggregate = _checked_aggregate_add(aggregate, len(executable_data))

    (
        transaction,
        created_work_dir,
        work_fd,
        work_identity,
        transaction_fd,
        transaction_identity,
    ) = _create_private_transaction(args.work_dir)
    host_outputs = None
    descriptors: list[tuple[int, tuple[int, ...], str, pathlib.Path]] = []
    try:
        project = transaction / "project"
        project.mkdir(mode=0o700)
        staged_paths: dict[pathlib.Path, pathlib.Path] = {}
        for artifact, (data, digest) in snapshots.items():
            staged = project.joinpath(*relative_paths[artifact].parts)
            staged.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
            _write_private_file(staged, data, 0o400)
            staged_paths[artifact] = staged
            descriptor = os.open(
                staged,
                os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_CLOEXEC", 0),
            )
            metadata = os.fstat(descriptor)
            staged_identity = _file_identity(metadata)
            if _hash_open_file(descriptor) != digest:
                os.close(descriptor)
                raise HostValidationError(
                    "staged KiCad artifact digest does not match its snapshot"
                )
            descriptors.append((descriptor, staged_identity, digest, staged))
        staged_source = staged_paths[source]
        staged_identity_map = staged_paths[identity_map]
        tools = transaction / "tools"
        tools.mkdir(mode=0o700)
        staged_normalizer = tools / "normalize_drc.py"
        _write_private_file(staged_normalizer, normalizer_data, 0o400)
        normalizer_descriptor = os.open(
            staged_normalizer,
            os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_CLOEXEC", 0),
        )
        normalizer_metadata = os.fstat(normalizer_descriptor)
        if _hash_open_file(normalizer_descriptor) != normalizer_digest:
            os.close(normalizer_descriptor)
            raise HostValidationError("staged normalizer digest does not match its snapshot")
        descriptors.append(
            (
                normalizer_descriptor,
                _file_identity(normalizer_metadata),
                normalizer_digest,
                staged_normalizer,
            )
        )
        staged_kicad = _stage_executable(args.kicad_cli, executable_data, transaction / "kicad")
        executable_descriptor = os.open(
            staged_kicad,
            os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_CLOEXEC", 0),
        )
        executable_metadata = os.fstat(executable_descriptor)
        if _hash_open_file(executable_descriptor) != executable_digest:
            os.close(executable_descriptor)
            raise HostValidationError("staged KiCad executable digest does not match its snapshot")
        descriptors.append(
            (
                executable_descriptor,
                _file_identity(executable_metadata),
                executable_digest,
                staged_kicad,
            )
        )

        def verify_project() -> None:
            for held, identity, digest, path in descriptors:
                _verify_staged_source(held, identity, digest, path)

        raw = transaction / "report.raw.json"
        normalized = transaction / "report.normalized.json"
        config = transaction / "config"
        home = transaction / "home"
        temp = transaction / "temp"
        for directory in (config, home, temp):
            directory.mkdir(mode=0o700)
        environment = {
            "HOME": str(home),
            "KICAD_CONFIG_HOME": str(config),
            "LC_ALL": "C",
            "PATH": "/usr/bin:/bin",
            "PYTHONDONTWRITEBYTECODE": "1",
            "PYTHONNOUSERSITE": "1",
            "TMPDIR": str(temp),
        }
        if args.kind == "erc":
            host_command = [
                str(staged_kicad),
                "sch",
                "erc",
                "--format",
                "json",
                "--severity-all",
                "--output",
                str(raw),
                str(staged_source),
            ]
        else:
            host_command = [
                str(staged_kicad),
                "pcb",
                "drc",
                "--format",
                "json",
                "--severity-all",
                "--schematic-parity",
                "--output",
                str(raw),
                str(staged_source),
            ]
        _recheck_private_transaction(
            transaction,
            work_fd,
            work_identity,
            transaction_fd,
            transaction_identity,
        )
        _run(host_command, environment, "KiCad host validation", [raw])
        _recheck_private_transaction(
            transaction,
            work_fd,
            work_identity,
            transaction_fd,
            transaction_identity,
        )
        verify_project()

        raw_data, _ = _read_source_handle(raw, MAX_AGGREGATE_BYTES - aggregate)
        aggregate = _checked_aggregate_add(aggregate, len(raw_data))
        raw.chmod(0o400)

        normalizer_command = [
            sys.executable,
            "-I",
            str(staged_normalizer),
            "--raw",
            str(raw),
            "--normalized",
            str(normalized),
            "--expected-major",
            str(args.expected_major),
            "--identity-map",
            str(staged_identity_map),
            "--source-artifact",
            str(staged_source),
            "--expected-source-sha256",
            source_digest,
        ]
        for value in args.allow_library_warning:
            normalizer_command.extend(("--allow-library-warning", value))
        for value in args.allow_ignored_check:
            normalizer_command.extend(("--allow-ignored-check", value))
        if args.retain_findings:
            normalizer_command.append("--retain-findings")
        _recheck_private_transaction(
            transaction,
            work_fd,
            work_identity,
            transaction_fd,
            transaction_identity,
        )
        _run(normalizer_command, environment, "KiCad report normalization", [normalized])
        _recheck_private_transaction(
            transaction,
            work_fd,
            work_identity,
            transaction_fd,
            transaction_identity,
        )
        verify_project()

        normalized_data, _ = _read_source_handle(normalized, MAX_AGGREGATE_BYTES - aggregate)
        aggregate = _checked_aggregate_add(aggregate, len(normalized_data))
        report = json.loads(normalized_data)
        if report.get("source_sha256") != source_digest:
            raise HostValidationError("normalized report lost the pre-execution source digest")
        host_outputs = (raw_data, normalized_data)
    finally:
        for descriptor, _, _, _ in descriptors:
            os.close(descriptor)
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
                raise HostValidationError("private transaction directory was replaced")
            os.rmdir(transaction.name, dir_fd=work_fd)
            if created_work_dir:
                _remove_empty_anchored_directory(args.work_dir, work_fd, work_identity)
        finally:
            os.close(transaction_fd)
            os.close(work_fd)
    if host_outputs is None:
        raise HostValidationError("host validation completed without a complete output pair")
    _publish_pair(args.raw_output, host_outputs[0], args.normalized_output, host_outputs[1])


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--kicad-cli", required=True, type=pathlib.Path)
    parser.add_argument("--normalizer", required=True, type=pathlib.Path)
    parser.add_argument("--kind", required=True, choices=("erc", "drc"))
    parser.add_argument("--source-artifact", required=True, type=pathlib.Path)
    parser.add_argument("--identity-map", required=True, type=pathlib.Path)
    parser.add_argument("--raw-output", required=True, type=pathlib.Path)
    parser.add_argument("--normalized-output", required=True, type=pathlib.Path)
    parser.add_argument("--work-dir", required=True, type=pathlib.Path)
    parser.add_argument("--expected-major", required=True, type=int)
    parser.add_argument("--allow-library-warning", action="append", default=[])
    parser.add_argument("--allow-ignored-check", action="append", default=[])
    parser.add_argument("--project-artifact", action="append", default=[], type=pathlib.Path)
    parser.add_argument("--retain-findings", action="store_true")
    args = parser.parse_args()
    try:
        run(args)
    except (OSError, json.JSONDecodeError, HostValidationError) as error:
        print(f"CircuitC KiCad host validation failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
