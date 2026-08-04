#!/usr/bin/env python3
"""Run KiCad and normalize its report against one immutable project snapshot."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import pathlib
import shutil
import stat
import subprocess
import sys
import tempfile

MAX_SOURCE_BYTES = 64 * 1024 * 1024


class HostValidationError(Exception):
    pass


def _file_identity(metadata: os.stat_result) -> tuple[int, int, int, int]:
    return metadata.st_dev, metadata.st_ino, metadata.st_size, metadata.st_mtime_ns


def _read_source_handle(path: pathlib.Path) -> tuple[bytes, str]:
    nofollow = getattr(os, "O_NOFOLLOW", 0)
    if nofollow == 0:
        raise HostValidationError("host cannot open the KiCad source without following links")
    descriptor = os.open(path, os.O_RDONLY | nofollow | getattr(os, "O_CLOEXEC", 0))
    try:
        before = os.fstat(descriptor)
        if not stat.S_ISREG(before.st_mode) or before.st_size > MAX_SOURCE_BYTES:
            raise HostValidationError("KiCad source is not a bounded regular file")
        chunks: list[bytes] = []
        total = 0
        while chunk := os.read(descriptor, 8192):
            total += len(chunk)
            if total > MAX_SOURCE_BYTES:
                raise HostValidationError("KiCad source exceeds the byte limit")
            chunks.append(chunk)
        after = os.fstat(descriptor)
        if _file_identity(after) != _file_identity(before) or total != before.st_size:
            raise HostValidationError("KiCad source changed while its snapshot was read")
        data = b"".join(chunks)
        return data, hashlib.sha256(data).hexdigest()
    finally:
        os.close(descriptor)


def _reject_project_symlinks(root: pathlib.Path) -> None:
    for directory, names, filenames in os.walk(root, followlinks=False):
        directory_path = pathlib.Path(directory)
        for name in [*names, *filenames]:
            if (directory_path / name).is_symlink():
                raise HostValidationError("KiCad project snapshot contains a symbolic link")


def _hash_open_file(descriptor: int) -> str:
    os.lseek(descriptor, 0, os.SEEK_SET)
    digest = hashlib.sha256()
    while chunk := os.read(descriptor, 8192):
        digest.update(chunk)
    return digest.hexdigest()


def _verify_staged_source(
    descriptor: int,
    identity: tuple[int, int, int, int],
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


def _run(command: list[str], environment: dict[str, str], label: str) -> None:
    process = subprocess.run(
        command,
        stdin=subprocess.DEVNULL,
        capture_output=True,
        check=False,
        env=environment,
    )
    if process.returncode != 0:
        stderr = process.stderr.decode("utf-8", errors="replace").strip()
        raise HostValidationError(f"{label} failed: {stderr}")


def _publish_new(path: pathlib.Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    try:
        with path.open("xb") as output:
            output.write(data)
            output.flush()
            os.fsync(output.fileno())
    except FileExistsError as error:
        raise HostValidationError(f"host validation output already exists: {path.name}") from error


def run(args: argparse.Namespace) -> None:
    source = args.source_artifact.absolute()
    identity_map = args.identity_map.absolute()
    try:
        identity_relative = identity_map.relative_to(source.parent)
    except ValueError as error:
        raise HostValidationError(
            "identity map must belong to the KiCad project directory"
        ) from error
    source_data, source_digest = _read_source_handle(source)
    _reject_project_symlinks(source.parent)

    args.work_dir.mkdir(parents=True, exist_ok=True)
    transaction = pathlib.Path(tempfile.mkdtemp(prefix="circuitc-kicad-", dir=args.work_dir))
    transaction.chmod(0o700)
    project = transaction / "project"
    shutil.copytree(source.parent, project)
    staged_source = project / source.name
    staged_source.write_bytes(source_data)
    staged_source.chmod(0o400)
    staged_identity_map = project / identity_relative
    if not staged_identity_map.is_file():
        raise HostValidationError("staged KiCad identity map is missing")

    descriptor = os.open(
        staged_source,
        os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_CLOEXEC", 0),
    )
    try:
        staged_metadata = os.fstat(descriptor)
        staged_identity = _file_identity(staged_metadata)
        if _hash_open_file(descriptor) != source_digest:
            raise HostValidationError("staged KiCad source digest does not match its snapshot")

        raw = transaction / "report.raw.json"
        normalized = transaction / "report.normalized.json"
        config = transaction / "config"
        config.mkdir(mode=0o700)
        environment = os.environ.copy()
        environment["KICAD_CONFIG_HOME"] = str(config)
        if args.kind == "erc":
            host_command = [
                str(args.kicad_cli),
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
                str(args.kicad_cli),
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
        _run(host_command, environment, "KiCad host validation")
        _verify_staged_source(descriptor, staged_identity, source_digest, staged_source)

        normalizer_command = [
            sys.executable,
            str(args.normalizer),
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
        _run(normalizer_command, environment, "KiCad report normalization")
        _verify_staged_source(descriptor, staged_identity, source_digest, staged_source)

        normalized_data = normalized.read_bytes()
        report = json.loads(normalized_data)
        if report.get("source_sha256") != source_digest:
            raise HostValidationError("normalized report lost the pre-execution source digest")
        _publish_new(args.raw_output, raw.read_bytes())
        _publish_new(args.normalized_output, normalized_data)
    finally:
        os.close(descriptor)


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
    args = parser.parse_args()
    try:
        run(args)
    except (OSError, json.JSONDecodeError, HostValidationError) as error:
        print(f"CircuitC KiCad host validation failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
