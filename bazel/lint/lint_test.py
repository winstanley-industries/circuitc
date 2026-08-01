"""Tests for CircuitC's lint orchestration."""

from __future__ import annotations

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path, PurePosixPath

from bazel.lint import lint


class ToolchainTest(unittest.TestCase):
    def test_python_minor_version_is_pinned(self) -> None:
        self.assertEqual(sys.version_info[:2], (3, 13))


class MatchingTest(unittest.TestCase):
    def test_file_checks_match_their_owned_inputs(self) -> None:
        checks = {check.name: check for check in lint.CHECKS}
        self.assertTrue(checks["buildifier"].matches(PurePosixPath("MODULE.bazel")))
        self.assertTrue(checks["buildifier"].matches(PurePosixPath("pkg/rules.bzl")))
        self.assertTrue(checks["ruff-check"].matches(PurePosixPath("tools/check.py")))
        self.assertTrue(checks["shellcheck"].matches(PurePosixPath("tools/bazel")))
        self.assertFalse(checks["shellcheck"].matches(PurePosixPath("README.md")))


class SelectionTest(unittest.TestCase):
    def test_default_selection_runs_every_check(self) -> None:
        self.assertEqual(lint.select_checks(None), lint.CHECKS)

    def test_explicit_selection_uses_registry_order(self) -> None:
        selected = lint.select_checks(["shellcheck", "clippy", "shellcheck"])
        self.assertEqual(tuple(check.name for check in selected), ("clippy", "shellcheck"))


class ArgumentTest(unittest.TestCase):
    def test_repository_wildcard_scope_is_accepted(self) -> None:
        args = lint.parse_args(["--only", "clippy", "//..."])
        self.assertEqual(args.only, ["clippy"])
        self.assertEqual(args.scope, ["//..."])

    def test_narrow_scope_is_rejected(self) -> None:
        with self.assertRaises(SystemExit):
            lint.parse_args(["//src/..."])


class DiscoveryTest(unittest.TestCase):
    def test_discovers_existing_tracked_and_untracked_files_in_stable_order(self) -> None:
        calls = []

        def runner(command, **kwargs):
            calls.append((command, kwargs))
            return subprocess.CompletedProcess(command, 0, stdout=b"z.py\0deleted.sh\0a.bzl\0")

        with tempfile.TemporaryDirectory() as temporary_directory:
            repo_root = Path(temporary_directory)
            (repo_root / "z.py").touch()
            (repo_root / "a.bzl").touch()
            files = lint.discover_files(repo_root, runner)

        self.assertEqual(files, (PurePosixPath("a.bzl"), PurePosixPath("z.py")))
        self.assertEqual(calls[0][0][:4], ["git", "-C", str(repo_root), "ls-files"])


class CommandConstructionTest(unittest.TestCase):
    def check(self, name: str) -> lint.Check:
        return next(check for check in lint.CHECKS if check.name == name)

    def test_bazel_check_does_not_append_files(self) -> None:
        command = lint.command_for(
            self.check("clippy"),
            fix=False,
            bazel_real="/tools/bazel-real",
            repo_root=Path("/repo"),
            files=(PurePosixPath("src/lib.rs"),),
        )
        self.assertEqual(command, ("/tools/bazel-real", "build", "//:clippy"))

    def test_rustfmt_fix_uses_rules_rust_formatter(self) -> None:
        command = lint.command_for(
            self.check("rustfmt"),
            fix=True,
            bazel_real="/tools/bazel-real",
            repo_root=Path("/repo"),
            files=(),
        )
        self.assertEqual(command, ("/tools/bazel-real", "run", "@rules_rust//:rustfmt"))

    def test_file_check_appends_only_matching_absolute_paths(self) -> None:
        command = lint.command_for(
            self.check("buildifier"),
            fix=False,
            bazel_real="/tools/bazel-real",
            repo_root=Path("/repo"),
            files=(PurePosixPath("BUILD.bazel"), PurePosixPath("tools/check.py")),
        )
        self.assertEqual(command[-1], "/repo/BUILD.bazel")
        self.assertNotIn("/repo/tools/check.py", command)

    def test_fix_without_fix_args_keeps_check_mode(self) -> None:
        command = lint.command_for(
            self.check("shellcheck"),
            fix=True,
            bazel_real="/tools/bazel-real",
            repo_root=Path("/repo"),
            files=(PurePosixPath("tools/bazel"),),
        )
        self.assertEqual(command[-2:], ("--format=gcc", "/repo/tools/bazel"))


class ExecutionTest(unittest.TestCase):
    def test_later_checks_run_after_a_failure(self) -> None:
        tool_calls = []

        def runner(command, **kwargs):
            if command[0] == "git":
                return subprocess.CompletedProcess(command, 0, stdout=b"")
            tool_calls.append(command)
            return subprocess.CompletedProcess(command, 7 if len(tool_calls) == 1 else 0)

        result = lint.run_checks(
            lint.select_checks(["rustfmt", "clippy"]),
            fix=False,
            bazel_real="/tools/bazel-real",
            repo_root=Path("/repo"),
            runner=runner,
        )

        self.assertEqual(result, 1)
        self.assertEqual(len(tool_calls), 2)


class MainTest(unittest.TestCase):
    def test_requires_wrapper_environment(self) -> None:
        self.assertEqual(lint.main([], environ={}), 2)

    def test_dispatches_using_wrapper_environment(self) -> None:
        calls = []

        def runner(command, **kwargs):
            calls.append(command)
            return subprocess.CompletedProcess(command, 0)

        result = lint.main(
            ["--only", "clippy"],
            environ={
                "CIRCUITC_BAZEL_REAL": "/tools/bazel-real",
                "CIRCUITC_REPO_ROOT": "/repo",
            },
            runner=runner,
        )

        self.assertEqual(result, 0)
        self.assertEqual(calls[0], ("/tools/bazel-real", "build", "//:clippy"))


if __name__ == "__main__":
    unittest.main()
