"""Bazel-pinned lint and static-analysis orchestration for CircuitC."""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path, PurePosixPath

Runner = Callable[..., subprocess.CompletedProcess[bytes]]


@dataclass(frozen=True)
class Check:
    """One repository check launched through the real Bazel binary."""

    name: str
    display_name: str
    args: tuple[str, ...]
    fix_args: tuple[str, ...] | None = None
    suffixes: frozenset[str] = field(default_factory=frozenset)
    filenames: frozenset[str] = field(default_factory=frozenset)
    paths: frozenset[str] = field(default_factory=frozenset)

    def is_file_check(self) -> bool:
        return bool(self.suffixes or self.filenames or self.paths)

    def matches(self, path: PurePosixPath) -> bool:
        return (
            str(path) in self.paths
            or path.name in self.filenames
            or path.suffix.lower() in self.suffixes
        )


STARLARK_FILENAMES = frozenset(
    {"BUILD", "BUILD.bazel", "MODULE.bazel", "WORKSPACE", "WORKSPACE.bazel"}
)

CHECKS = (
    Check(
        name="rustfmt",
        display_name="rustfmt (first-party Rust)",
        args=("test", "//:rustfmt_test"),
        fix_args=("run", "@rules_rust//:rustfmt"),
    ),
    Check(
        name="clippy",
        display_name="Clippy (first-party Rust)",
        args=("build", "//:clippy"),
    ),
    Check(
        name="buildifier",
        display_name="Buildifier (Bazel/Starlark)",
        args=(
            "run",
            "@buildifier_prebuilt//:buildifier",
            "--",
            "-mode=check",
            "-lint=warn",
        ),
        fix_args=(
            "run",
            "@buildifier_prebuilt//:buildifier",
            "--",
            "-mode=fix",
            "-lint=fix",
        ),
        suffixes=frozenset({".bzl"}),
        filenames=STARLARK_FILENAMES,
    ),
    Check(
        name="ruff-check",
        display_name="Ruff check (Python)",
        args=("run", "@ruff", "--", "check"),
        fix_args=("run", "@ruff", "--", "check", "--fix"),
        suffixes=frozenset({".py", ".pyi"}),
    ),
    Check(
        name="ruff-format",
        display_name="Ruff format (Python)",
        args=("run", "@ruff", "--", "format", "--check"),
        fix_args=("run", "@ruff", "--", "format"),
        suffixes=frozenset({".py", ".pyi"}),
    ),
    Check(
        name="shellcheck",
        display_name="ShellCheck (shell)",
        args=("run", "@rules_shellcheck//:shellcheck", "--", "--format=gcc"),
        suffixes=frozenset({".bash", ".sh"}),
        paths=frozenset({"tools/bazel"}),
    ),
)


def parse_args(argv: Sequence[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        prog="bazel lint",
        description="Run CircuitC's Bazel-pinned repository checks.",
    )
    parser.add_argument(
        "--fix",
        action="store_true",
        help="rewrite files in place where the selected check supports it",
    )
    parser.add_argument(
        "--only",
        action="append",
        choices=tuple(check.name for check in CHECKS),
        metavar="CHECK",
        help="run one named check; may be repeated",
    )
    parser.add_argument(
        "scope",
        nargs="*",
        help="optional Bazel scope; only the repository-wide //... contract is supported",
    )
    args = parser.parse_args(argv)
    if args.scope not in ([], ["//..."]):
        parser.error("only the repository-wide //... scope is supported")
    return args


def select_checks(only: Sequence[str] | None) -> tuple[Check, ...]:
    if not only:
        return CHECKS
    selected = set(only)
    return tuple(check for check in CHECKS if check.name in selected)


def discover_files(repo_root: Path, runner: Runner = subprocess.run) -> tuple[PurePosixPath, ...]:
    result = runner(
        [
            "git",
            "-C",
            str(repo_root),
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ],
        check=True,
        stdout=subprocess.PIPE,
    )
    files = (PurePosixPath(os.fsdecode(raw)) for raw in result.stdout.split(b"\0") if raw)
    existing = (path for path in files if (repo_root / str(path)).is_file())
    return tuple(sorted(existing, key=str))


def command_for(
    check: Check,
    *,
    fix: bool,
    bazel_real: str,
    repo_root: Path,
    files: Sequence[PurePosixPath],
) -> tuple[str, ...] | None:
    args = check.fix_args if fix and check.fix_args else check.args
    if not check.is_file_check():
        return (bazel_real, *args)
    matched = tuple(str(repo_root / str(path)) for path in files if check.matches(path))
    if not matched:
        return None
    return (bazel_real, *args, *matched)


def run_checks(
    checks: Sequence[Check],
    *,
    fix: bool,
    bazel_real: str,
    repo_root: Path,
    runner: Runner = subprocess.run,
) -> int:
    files = (
        discover_files(repo_root, runner) if any(check.is_file_check() for check in checks) else ()
    )
    failed: list[Check] = []

    for check in checks:
        command = command_for(
            check,
            fix=fix,
            bazel_real=bazel_real,
            repo_root=repo_root,
            files=files,
        )
        if command is None:
            print(f"==> {check.display_name}: no matching files", flush=True)
            continue

        mode = "fix" if fix and check.fix_args else "check"
        print(f"==> {check.display_name} ({mode})", flush=True)
        result = runner(command, check=False, cwd=repo_root)
        if result.returncode != 0:
            failed.append(check)

    for check in failed:
        print(f"FAILED: {check.display_name}", file=sys.stderr, flush=True)
    return 1 if failed else 0


def main(
    argv: Sequence[str] | None = None,
    *,
    environ: Mapping[str, str] = os.environ,
    runner: Runner = subprocess.run,
) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)

    bazel_real = environ.get("CIRCUITC_BAZEL_REAL")
    repo_root = environ.get("CIRCUITC_REPO_ROOT")
    if not bazel_real or not repo_root:
        print("bazel lint must be launched through CircuitC's Bazelisk wrapper", file=sys.stderr)
        return 2

    try:
        return run_checks(
            select_checks(args.only),
            fix=args.fix,
            bazel_real=bazel_real,
            repo_root=Path(repo_root),
            runner=runner,
        )
    except subprocess.CalledProcessError as error:
        return error.returncode
