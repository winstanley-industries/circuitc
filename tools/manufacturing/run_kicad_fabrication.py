#!/usr/bin/env python3
"""Export one exact CircuitC board through an isolated KiCad 10.0.5 host."""

from __future__ import annotations

import argparse
import ctypes
import datetime
import errno
import hashlib
import json
import os
import pathlib
import re
import secrets
import selectors
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time

MAX_SOURCE_BYTES = 64 * 1024 * 1024
MAX_OUTPUT_BYTES = 64 * 1024 * 1024
MAX_AGGREGATE_BYTES = 256 * 1024 * 1024
MAX_STDIO_BYTES = 1024 * 1024
TIMEOUT_SECONDS = 120
KICAD_VERSION = "10.0.5"
DESIGN_RE = re.compile(r"[A-Za-z0-9][A-Za-z0-9_-]{0,127}\Z")
SHA256_RE = re.compile(r"[0-9a-f]{64}\Z")
DATE_RE = re.compile(r"[0-9]{4}-[0-9]{2}-[0-9]{2}\Z")


class HostExportError(Exception):
    pass


def _unique_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        if key in result:
            raise HostExportError(f"fabrication request contains duplicate key: {key}")
        result[key] = value
    return result


def _load_request(path: pathlib.Path, board_bytes: bytes) -> tuple[dict[str, object], bytes]:
    descriptor, identity = _open_regular(path.absolute(), MAX_SOURCE_BYTES)
    try:
        request_bytes = _read_descriptor(descriptor, MAX_SOURCE_BYTES)
        _verify_open_path(descriptor, identity, path.absolute())
    finally:
        os.close(descriptor)
    try:
        request = json.loads(request_bytes, object_pairs_hook=_unique_object)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise HostExportError(f"fabrication request is not strict JSON: {error}") from error
    if not isinstance(request, dict):
        raise HostExportError("fabrication request root is not an object")
    design_name = request.get("design_name")
    if not isinstance(design_name, str) or DESIGN_RE.fullmatch(design_name) is None:
        raise HostExportError("fabrication request Design name is not a safe artifact stem")
    layers = [
        (0, "F.Cu", "Copper,L1,Top", "Copper,L1,Top", "Positive", "F_Cu"),
        (1, "F.Mask", "Soldermask,Top", "SolderMask,Top", "Negative", "F_Mask"),
        (2, "B.Cu", "Copper,L2,Bot", "Copper,L2,Bot", "Positive", "B_Cu"),
        (3, "B.Mask", "Soldermask,Bot", "SolderMask,Bot", "Negative", "B_Mask"),
        (5, "F.SilkS", "Legend,Top", "Legend,Top", "Positive", "F_Silkscreen"),
        (7, "B.SilkS", "Legend,Bot", "Legend,Bot", "Positive", "B_Silkscreen"),
        (13, "F.Paste", "Paste,Top", "SolderPaste,Top", "Positive", "F_Paste"),
        (15, "B.Paste", "Paste,Bot", "SolderPaste,Bot", "Positive", "B_Paste"),
        (25, "Edge.Cuts", "Profile,NP", "Profile", "Positive", "Edge_Cuts"),
    ]
    expected_layers = [
        {
            "layer_id": layer_id,
            "layer_name": layer_name,
            "file_function": function,
            "job_file_function": job_function,
            "file_polarity": polarity,
            "path": f"gerber/{design_name}-{filename_layer}.gbr",
        }
        for layer_id, layer_name, function, job_function, polarity, filename_layer in layers
    ]
    expected_profile = {
        "gerber": {
            "format": "x2",
            "precision": 6,
            "net_attributes": True,
            "protel_extensions": False,
            "origin": "page",
            "board_plot_params": False,
            "layers": expected_layers,
        },
        "drill": {
            "format": "excellon",
            "origin": "absolute",
            "units": "mm",
            "zero_format": "decimal",
            "oval_format": "alternate",
            "mirror_y": False,
            "minimal_header": False,
            "separate_plated": True,
            "generate_map": False,
            "generate_report": False,
            "generate_tenting": False,
        },
        "position": {
            "format": "csv",
            "units": "mm",
            "side": "both",
            "origin": "page",
            "bottom_negate_x": False,
            "smd_only": False,
            "exclude_through_hole": False,
            "exclude_dnp": False,
            "variant": None,
        },
        "resources": {
            "timeout_ms": 120000,
            "stdout_bytes": 1048576,
            "stderr_bytes": 1048576,
            "file_bytes": 67108864,
            "aggregate_bytes": 268435456,
            "primary_rows": 10000,
            "diagnostics": 256,
        },
    }
    expected_outputs = [
        {"role": f"gerber_layer_{layer['layer_id']}", "path": layer["path"]}
        for layer in expected_layers
    ] + [
        {"role": "gerber_job", "path": f"gerber/{design_name}-job.gbrjob"},
        {
            "role": "drill_non_plated_through",
            "path": f"drill/{design_name}-NPTH.drl",
        },
        {
            "role": "drill_plated_through",
            "path": f"drill/{design_name}-PTH.drl",
        },
        {"role": "position_all", "path": f"position/{design_name}-all-pos.csv"},
    ]
    string_fields = [
        "analysis_path",
        "assertion_path",
        "variant_path",
        "catalog_evaluated_on",
    ]
    if any(not isinstance(request.get(field), str) for field in string_fields):
        raise HostExportError("fabrication request identity fields have the wrong type")
    digest_fields = [
        "variant_identity_sha256",
        "product_input_sha256",
        "product_resolution_sha256",
        "placement_sha256",
    ]
    if any(
        not isinstance(request.get(field), str) or SHA256_RE.fullmatch(request[field]) is None
        for field in digest_fields
    ):
        raise HostExportError("fabrication request contains an invalid SHA-256 identity")
    try:
        if DATE_RE.fullmatch(request["catalog_evaluated_on"]) is None:
            raise ValueError
        datetime.date.fromisoformat(request["catalog_evaluated_on"])
    except ValueError as error:
        raise HostExportError("fabrication request evaluation date is invalid") from error
    expected_board = {
        "path": f"{design_name}.kicad_pcb",
        "sha256": _sha256(board_bytes),
    }
    preimage = {
        "design_name": design_name,
        "analysis_path": request["analysis_path"],
        "assertion_path": request["assertion_path"],
        "variant_path": request["variant_path"],
        "variant_identity_sha256": request["variant_identity_sha256"],
        "product_input_sha256": request["product_input_sha256"],
        "product_resolution_sha256": request["product_resolution_sha256"],
        "placement_sha256": request["placement_sha256"],
        "catalog_evaluated_on": request["catalog_evaluated_on"],
        "kicad_pcb": expected_board,
        "expected_adapter": "kicad",
        "expected_major": 10,
        "expected_version": KICAD_VERSION,
        "export_profile": expected_profile,
        "outputs": expected_outputs,
    }
    preimage_bytes = json.dumps(preimage, ensure_ascii=False, separators=(",", ":")).encode()
    fabrication_identity = hashlib.sha256(
        b"CIRCUITC-FABRICATION-IDENTITY-V1\0" + preimage_bytes
    ).hexdigest()
    expected_request = {
        "schema_name": "circuitc.fabrication_request",
        "schema_version": 1,
        "design_name": design_name,
        "analysis_path": request["analysis_path"],
        "assertion_path": request["assertion_path"],
        "variant_path": request["variant_path"],
        "variant_identity_sha256": request["variant_identity_sha256"],
        "product_input_sha256": request["product_input_sha256"],
        "product_resolution_sha256": request["product_resolution_sha256"],
        "placement_sha256": request["placement_sha256"],
        "catalog_evaluated_on": request["catalog_evaluated_on"],
        "kicad_pcb": expected_board,
        "expected_adapter": "kicad",
        "expected_major": 10,
        "expected_version": KICAD_VERSION,
        "fabrication_identity_sha256": fabrication_identity,
        "export_profile": expected_profile,
        "outputs": expected_outputs,
    }
    canonical = (
        json.dumps(expected_request, ensure_ascii=False, separators=(",", ":")) + "\n"
    ).encode()
    if request != expected_request or request_bytes != canonical:
        raise HostExportError("fabrication request does not match the board or fixed KiCad profile")
    return request, request_bytes


def _identity(metadata: os.stat_result) -> tuple[int, int, int, int]:
    return metadata.st_dev, metadata.st_ino, metadata.st_size, metadata.st_mtime_ns


def _open_regular(path: pathlib.Path, maximum: int) -> tuple[int, tuple[int, int, int, int]]:
    nofollow = getattr(os, "O_NOFOLLOW", 0)
    if nofollow == 0:
        raise HostExportError("host cannot open fabrication inputs without following links")
    try:
        descriptor = os.open(path, os.O_RDONLY | nofollow | getattr(os, "O_CLOEXEC", 0))
    except OSError as error:
        raise HostExportError(f"input is not a bounded regular file: {path.name}") from error
    metadata = os.fstat(descriptor)
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_size > maximum:
        os.close(descriptor)
        raise HostExportError(f"input is not a bounded regular file: {path.name}")
    return descriptor, _identity(metadata)


def _read_descriptor(descriptor: int, maximum: int) -> bytes:
    os.lseek(descriptor, 0, os.SEEK_SET)
    chunks: list[bytes] = []
    total = 0
    while chunk := os.read(descriptor, 8192):
        total += len(chunk)
        if total > maximum:
            raise HostExportError("input changed beyond its authenticated byte limit")
        chunks.append(chunk)
    return b"".join(chunks)


def _sha256(contents: bytes) -> str:
    return hashlib.sha256(contents).hexdigest()


def _verify_open_path(
    descriptor: int,
    expected: tuple[int, int, int, int],
    path: pathlib.Path,
) -> None:
    descriptor_metadata = os.fstat(descriptor)
    path_metadata = path.lstat()
    if (
        _identity(descriptor_metadata) != expected
        or _identity(path_metadata) != expected
        or not stat.S_ISREG(path_metadata.st_mode)
    ):
        raise HostExportError(f"authenticated input changed during host export: {path.name}")


def _kill_process_group(process: subprocess.Popen[bytes]) -> None:
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass


def _run_process(
    command: list[str], environment: dict[str, str], label: str
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
        raise HostExportError(f"{label} did not expose bounded process streams")
    streams = selectors.DefaultSelector()
    streams.register(process.stdout, selectors.EVENT_READ, "stdout")
    streams.register(process.stderr, selectors.EVENT_READ, "stderr")
    captured = {"stdout": bytearray(), "stderr": bytearray()}
    deadline = time.monotonic() + TIMEOUT_SECONDS
    try:
        while streams.get_map():
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                _kill_process_group(process)
                process.wait()
                raise HostExportError(f"{label} exceeded the {TIMEOUT_SECONDS}s deadline")
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
                    raise HostExportError(f"{label} exceeded the bounded stdout/stderr budget")
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            _kill_process_group(process)
            process.wait()
            raise HostExportError(f"{label} exceeded the {TIMEOUT_SECONDS}s deadline")
        returncode = process.wait(timeout=remaining)
    except subprocess.TimeoutExpired as error:
        _kill_process_group(process)
        process.wait()
        raise HostExportError(f"{label} exceeded the {TIMEOUT_SECONDS}s deadline") from error
    finally:
        streams.close()
        process.stdout.close()
        process.stderr.close()
    stdout = bytes(captured["stdout"])
    stderr = bytes(captured["stderr"])
    if returncode != 0:
        detail = stderr.decode("utf-8", errors="replace").strip()
        raise HostExportError(f"{label} failed with exit {returncode}: {detail}")
    return stdout, stderr


def _run(
    command: list[str],
    environment: dict[str, str],
    label: str,
    board_descriptor: int,
    board_identity: tuple[int, int, int, int],
    board_path: pathlib.Path,
    executable_descriptor: int,
    executable_identity: tuple[int, int, int, int],
    executable_path: pathlib.Path,
) -> None:
    _verify_open_path(board_descriptor, board_identity, board_path)
    _verify_open_path(executable_descriptor, executable_identity, executable_path)
    _run_process(command, environment, label)
    _verify_open_path(board_descriptor, board_identity, board_path)
    _verify_open_path(executable_descriptor, executable_identity, executable_path)


def _preflight_exact_output(directory: int, name: str) -> tuple[int, int, int, int]:
    try:
        descriptor = os.open(
            name,
            os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_CLOEXEC", 0),
            dir_fd=directory,
        )
    except OSError as error:
        raise HostExportError(f"host output is not a bounded regular file: {name}") from error
    try:
        before = os.fstat(descriptor)
        if (
            not stat.S_ISREG(before.st_mode)
            or before.st_nlink != 1
            or before.st_size > MAX_OUTPUT_BYTES
        ):
            raise HostExportError(f"host output is not a bounded regular file: {name}")
        return _identity(before)
    finally:
        os.close(descriptor)


def _read_exact_output(
    directory: int, name: str, expected_identity: tuple[int, int, int, int]
) -> bytes:
    try:
        descriptor = os.open(
            name,
            os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_CLOEXEC", 0),
            dir_fd=directory,
        )
    except OSError as error:
        raise HostExportError(f"host output is not a bounded regular file: {name}") from error
    try:
        before = os.fstat(descriptor)
        if (
            not stat.S_ISREG(before.st_mode)
            or before.st_nlink != 1
            or _identity(before) != expected_identity
        ):
            raise HostExportError(f"host output changed after aggregate preflight: {name}")
        contents = _read_descriptor(descriptor, MAX_OUTPUT_BYTES)
        after = os.fstat(descriptor)
        if _identity(after) != _identity(before) or len(contents) != before.st_size:
            raise HostExportError(f"host output changed while it was read: {name}")
        return contents
    finally:
        os.close(descriptor)


def _checked_aggregate(lengths: list[int]) -> int:
    total = 0
    for length in lengths:
        if length < 0 or length > MAX_OUTPUT_BYTES:
            raise HostExportError("host output is not a bounded regular file")
        total += length
        if total > MAX_AGGREGATE_BYTES:
            raise HostExportError("host output aggregate exceeds the 256 MiB byte limit")
    return total


def _open_anchored_directory(path: pathlib.Path) -> int:
    absolute = path.absolute()
    flags = (
        os.O_RDONLY
        | getattr(os, "O_DIRECTORY", 0)
        | getattr(os, "O_NOFOLLOW", 0)
        | getattr(os, "O_CLOEXEC", 0)
    )
    descriptor = os.open(absolute.anchor, flags)
    try:
        for component in absolute.parts[1:]:
            next_descriptor = os.open(component, flags, dir_fd=descriptor)
            os.close(descriptor)
            descriptor = next_descriptor
        return descriptor
    except BaseException:
        os.close(descriptor)
        raise


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
        raise HostExportError("host has no atomic no-replace directory publication primitive")
    if result != 0:
        error_number = ctypes.get_errno()
        if error_number == errno.EEXIST:
            raise HostExportError(f"fabrication output already exists: {destination}")
        raise HostExportError(f"atomic fabrication publication failed: {os.strerror(error_number)}")


def _publish_exact(output_root: pathlib.Path, outputs: dict[str, bytes]) -> None:
    parent_fd = _open_anchored_directory(output_root.parent)
    final_name = output_root.name
    temporary_name = f".{final_name}.circuitc-{secrets.token_hex(12)}"
    os.mkdir(temporary_name, mode=0o700, dir_fd=parent_fd)
    root_fd = os.open(
        temporary_name,
        os.O_RDONLY
        | getattr(os, "O_DIRECTORY", 0)
        | getattr(os, "O_NOFOLLOW", 0)
        | getattr(os, "O_CLOEXEC", 0),
        dir_fd=parent_fd,
    )
    directory_fds: dict[str, int] = {}
    published = False
    try:
        for directory in sorted({path.split("/", 1)[0] for path in outputs}):
            os.mkdir(directory, mode=0o700, dir_fd=root_fd)
            directory_fds[directory] = os.open(
                directory,
                os.O_RDONLY
                | getattr(os, "O_DIRECTORY", 0)
                | getattr(os, "O_NOFOLLOW", 0)
                | getattr(os, "O_CLOEXEC", 0),
                dir_fd=root_fd,
            )
        for relative, contents in sorted(outputs.items()):
            directory, basename = relative.split("/", 1)
            descriptor = os.open(
                basename,
                os.O_WRONLY
                | os.O_CREAT
                | os.O_EXCL
                | getattr(os, "O_NOFOLLOW", 0)
                | getattr(os, "O_CLOEXEC", 0),
                0o600,
                dir_fd=directory_fds[directory],
            )
            try:
                view = memoryview(contents)
                while view:
                    written = os.write(descriptor, view)
                    view = view[written:]
                os.fsync(descriptor)
            finally:
                os.close(descriptor)
        for descriptor in directory_fds.values():
            os.fsync(descriptor)
        os.fsync(root_fd)
        _rename_noreplace(parent_fd, temporary_name, final_name)
        os.fsync(parent_fd)
        published = True
    finally:
        for descriptor in directory_fds.values():
            os.close(descriptor)
        os.close(root_fd)
        if not published:
            temporary_path = output_root.parent / temporary_name
            shutil.rmtree(temporary_path, ignore_errors=True)
        os.close(parent_fd)


def run(args: argparse.Namespace) -> None:
    board_path = args.board.absolute()
    executable_path = args.kicad_cli.absolute()
    board_descriptor, board_identity = _open_regular(board_path, MAX_SOURCE_BYTES)
    executable_descriptor, executable_identity = _open_regular(executable_path, 512 * 1024 * 1024)
    try:
        board_bytes = _read_descriptor(board_descriptor, MAX_SOURCE_BYTES)
        executable_bytes = _read_descriptor(executable_descriptor, 512 * 1024 * 1024)
        request, request_bytes = _load_request(args.request, board_bytes)
        design_name = request["design_name"]
        if not isinstance(design_name, str):
            raise HostExportError("fabrication request Design name has the wrong type")
        _verify_open_path(board_descriptor, board_identity, board_path)
        args.work_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
        transaction = pathlib.Path(
            tempfile.mkdtemp(prefix="circuitc-fabrication-", dir=args.work_dir)
        )
        transaction.chmod(0o700)
        if (
            executable_path.parent.name == "MacOS"
            and executable_path.parent.parent.name == "Contents"
        ):
            original_contents = executable_path.parent.parent
            staged_contents = transaction / "host" / "KiCad.app" / "Contents"
            staged_executable = staged_contents / "MacOS" / "kicad-cli"
            staged_executable.parent.mkdir(mode=0o700, parents=True)
            for resource_name in (
                "Applications",
                "Frameworks",
                "Info.plist",
                "PlugIns",
                "Resources",
                "SharedSupport",
            ):
                resource = original_contents / resource_name
                if resource.exists():
                    (staged_contents / resource_name).symlink_to(resource)
        else:
            staged_executable = transaction / "host" / "kicad-cli"
            staged_executable.parent.mkdir(mode=0o700)
        staged_executable_fd = os.open(
            staged_executable,
            os.O_WRONLY
            | os.O_CREAT
            | os.O_EXCL
            | getattr(os, "O_NOFOLLOW", 0)
            | getattr(os, "O_CLOEXEC", 0),
            0o500,
        )
        try:
            executable_view = memoryview(executable_bytes)
            while executable_view:
                written = os.write(staged_executable_fd, executable_view)
                executable_view = executable_view[written:]
            os.fsync(staged_executable_fd)
        finally:
            os.close(staged_executable_fd)
        _verify_open_path(executable_descriptor, executable_identity, executable_path)
        staged_executable_descriptor, staged_executable_identity = _open_regular(
            staged_executable, 512 * 1024 * 1024
        )
        if _read_descriptor(staged_executable_descriptor, 512 * 1024 * 1024) != executable_bytes:
            os.close(staged_executable_descriptor)
            raise HostExportError("private executable snapshot does not match authenticated input")
        staged_board = transaction / f"{design_name}.kicad_pcb"
        staged_fd = os.open(
            staged_board,
            os.O_WRONLY
            | os.O_CREAT
            | os.O_EXCL
            | getattr(os, "O_NOFOLLOW", 0)
            | getattr(os, "O_CLOEXEC", 0),
            0o400,
        )
        try:
            view = memoryview(board_bytes)
            while view:
                written = os.write(staged_fd, view)
                view = view[written:]
            os.fsync(staged_fd)
        finally:
            os.close(staged_fd)
        staged_descriptor, staged_identity = _open_regular(staged_board, MAX_SOURCE_BYTES)
        try:
            if _read_descriptor(staged_descriptor, MAX_SOURCE_BYTES) != board_bytes:
                raise HostExportError("private board snapshot does not match authenticated input")
            gerber = transaction / "gerber"
            drill = transaction / "drill"
            position = transaction / "position"
            config = transaction / "config"
            temp = transaction / "temp"
            home = transaction / "home"
            for directory in (gerber, drill, position, config, temp, home):
                directory.mkdir(mode=0o700)
            environment = {
                "HOME": str(home),
                "KICAD_CONFIG_HOME": str(config),
                "LC_ALL": "C",
                "PATH": "/usr/bin:/bin",
                "TMPDIR": str(temp),
            }
            version_stdout, _ = _run_process(
                [str(staged_executable), "--version"],
                environment,
                "KiCad version probe",
            )
            _verify_open_path(
                staged_executable_descriptor,
                staged_executable_identity,
                staged_executable,
            )
            if version_stdout.decode("utf-8", errors="strict").strip() != KICAD_VERSION:
                raise HostExportError(f"fabrication host must be exactly KiCad {KICAD_VERSION}")
            common = (
                staged_descriptor,
                staged_identity,
                staged_board,
                staged_executable_descriptor,
                staged_executable_identity,
                staged_executable,
            )
            _run(
                [
                    str(staged_executable),
                    "pcb",
                    "export",
                    "gerbers",
                    "--output",
                    str(gerber),
                    "--layers",
                    ",".join(
                        layer["layer_name"]
                        for layer in request["export_profile"]["gerber"]["layers"]
                    ),
                    "--precision",
                    str(request["export_profile"]["gerber"]["precision"]),
                    "--no-protel-ext",
                    str(staged_board),
                ],
                environment,
                "KiCad Gerber export",
                *common,
            )
            _run(
                [
                    str(staged_executable),
                    "pcb",
                    "export",
                    "drill",
                    "--output",
                    f"{drill}/",
                    "--format",
                    "excellon",
                    "--drill-origin",
                    "absolute",
                    "--excellon-zeros-format",
                    "decimal",
                    "--excellon-oval-format",
                    "alternate",
                    "--excellon-units",
                    "mm",
                    "--excellon-separate-th",
                    str(staged_board),
                ],
                environment,
                "KiCad drill export",
                *common,
            )
            position_name = f"{design_name}-all-pos.csv"
            _run(
                [
                    str(staged_executable),
                    "pcb",
                    "export",
                    "pos",
                    "--output",
                    str(position / position_name),
                    "--side",
                    "both",
                    "--format",
                    "csv",
                    "--units",
                    "mm",
                    str(staged_board),
                ],
                environment,
                "KiCad position export",
                *common,
            )
            _verify_open_path(staged_descriptor, staged_identity, staged_board)
            _verify_open_path(board_descriptor, board_identity, board_path)

            gerber_names = {
                f"{design_name}-F_Cu.gbr",
                f"{design_name}-F_Mask.gbr",
                f"{design_name}-B_Cu.gbr",
                f"{design_name}-B_Mask.gbr",
                f"{design_name}-F_Silkscreen.gbr",
                f"{design_name}-B_Silkscreen.gbr",
                f"{design_name}-F_Paste.gbr",
                f"{design_name}-B_Paste.gbr",
                f"{design_name}-Edge_Cuts.gbr",
                f"{design_name}-job.gbrjob",
            }
            drill_names = {
                f"{design_name}-PTH.drl",
                f"{design_name}-NPTH.drl",
            }
            gerber_fd = _open_anchored_directory(gerber)
            drill_fd = _open_anchored_directory(drill)
            position_fd = _open_anchored_directory(position)
            try:
                if set(os.listdir(gerber_fd)) != gerber_names:
                    raise HostExportError("KiCad Gerber output inventory is not exact")
                if set(os.listdir(drill_fd)) != drill_names:
                    raise HostExportError("KiCad drill output inventory is not exact")
                if set(os.listdir(position_fd)) != {position_name}:
                    raise HostExportError("KiCad position output inventory is not exact")
                output_locations = [
                    *[(f"gerber/{name}", gerber_fd, name) for name in sorted(gerber_names)],
                    *[(f"drill/{name}", drill_fd, name) for name in sorted(drill_names)],
                    (f"position/{position_name}", position_fd, position_name),
                ]
                preflight = {
                    path: _preflight_exact_output(directory, name)
                    for path, directory, name in output_locations
                }
                _checked_aggregate([identity[2] for identity in preflight.values()])
                outputs: dict[str, bytes] = {}
                actual_lengths: list[int] = []
                for path, directory, name in output_locations:
                    contents = _read_exact_output(directory, name, preflight[path])
                    actual_lengths.append(len(contents))
                    _checked_aggregate(actual_lengths)
                    outputs[path] = contents
                if (
                    set(os.listdir(gerber_fd)) != gerber_names
                    or set(os.listdir(drill_fd)) != drill_names
                    or set(os.listdir(position_fd)) != {position_name}
                ):
                    raise HostExportError(
                        "KiCad output inventory changed while it was authenticated"
                    )
            finally:
                os.close(gerber_fd)
                os.close(drill_fd)
                os.close(position_fd)
            receipt = {
                "schema_name": "circuitc.kicad_fabrication_receipt",
                "schema_version": 1,
                "request_sha256": _sha256(request_bytes),
                "board_sha256": _sha256(board_bytes),
                "executable_sha256": _sha256(executable_bytes),
                "outputs": [
                    {"path": path, "sha256": _sha256(contents)}
                    for path, contents in sorted(outputs.items())
                ],
            }
            outputs["receipt/host.json"] = (
                json.dumps(receipt, ensure_ascii=False, separators=(",", ":")) + "\n"
            ).encode()
            _publish_exact(args.output_dir.absolute(), outputs)
        finally:
            os.close(staged_descriptor)
            os.close(staged_executable_descriptor)
        shutil.rmtree(transaction)
    finally:
        os.close(board_descriptor)
        os.close(executable_descriptor)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--kicad-cli", required=True, type=pathlib.Path)
    parser.add_argument("--request", required=True, type=pathlib.Path)
    parser.add_argument("--board", required=True, type=pathlib.Path)
    parser.add_argument("--output-dir", required=True, type=pathlib.Path)
    parser.add_argument("--work-dir", required=True, type=pathlib.Path)
    args = parser.parse_args()
    try:
        run(args)
    except (OSError, HostExportError, subprocess.TimeoutExpired, UnicodeDecodeError) as error:
        print(f"CircuitC KiCad fabrication export failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
