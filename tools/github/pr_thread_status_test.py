"""Tests for CircuitC's exact-head PR review-thread helper."""

from __future__ import annotations

import io
import json
import subprocess
import sys
import unittest
from unittest import mock

from tools.github import pr_thread_status


def thread(
    identifier: str,
    *,
    resolved: bool,
    outdated: bool,
    body: str = "body",
) -> dict:
    return {
        "id": identifier,
        "isResolved": resolved,
        "isOutdated": outdated,
        "path": f"src/{identifier}.rs",
        "line": 7,
        "originalLine": 6,
        "comments": {
            "nodes": [
                {
                    "author": {"login": "reviewer"},
                    "body": body,
                    "url": f"https://example.test/{identifier}",
                    "createdAt": "2026-08-02T00:00:00Z",
                }
            ]
        },
    }


def page(
    nodes: list[dict],
    *,
    head: str = "head-1",
    has_next: bool = False,
    cursor: str | None = None,
) -> str:
    return json.dumps(
        {
            "data": {
                "repository": {
                    "pullRequest": {
                        "headRefOid": head,
                        "reviewThreads": {
                            "nodes": nodes,
                            "pageInfo": {
                                "hasNextPage": has_next,
                                "endCursor": cursor,
                            },
                        },
                    }
                }
            }
        }
    )


class ToolchainTest(unittest.TestCase):
    def test_python_minor_version_is_pinned(self) -> None:
        self.assertEqual(sys.version_info[:2], (3, 13))


class RepositoryTest(unittest.TestCase):
    def test_parse_repo_accepts_numeric_names_without_coercion(self) -> None:
        self.assertEqual(pr_thread_status.parse_repo("123/2048"), ("123", "2048"))

    def test_parse_repo_rejects_invalid_shapes(self) -> None:
        for value in ("", "owner", "a/b/c", "owner/", "/repo"):
            with self.subTest(value=value), self.assertRaises(ValueError):
                pr_thread_status.parse_repo(value)

    def test_graphql_uses_raw_string_fields(self) -> None:
        calls = []

        def runner(arguments: list[str]) -> str:
            calls.append(arguments)
            return page([])

        pr_thread_status.graphql_page("123", "2048", 7, "@cursor", runner)

        arguments = calls[0]
        self.assertEqual(arguments[arguments.index("owner=123") - 1], "-f")
        self.assertEqual(arguments[arguments.index("name=2048") - 1], "-f")
        self.assertEqual(arguments[arguments.index("number=7") - 1], "-F")
        self.assertEqual(arguments[arguments.index("after=@cursor") - 1], "-f")


class CollectionTest(unittest.TestCase):
    def test_paginates_and_counts_mixed_thread_states(self) -> None:
        responses = iter(
            [
                page(
                    [
                        thread("current", resolved=False, outdated=False),
                        thread("resolved", resolved=True, outdated=False),
                    ],
                    has_next=True,
                    cursor="page-2",
                ),
                page([thread("outdated", resolved=False, outdated=True)]),
            ]
        )
        calls = []

        def runner(arguments: list[str]) -> str:
            calls.append(arguments)
            return next(responses)

        report = pr_thread_status.collect("owner/repo", 3, runner)

        self.assertEqual(
            report["counts"],
            {
                "total": 3,
                "resolved": 1,
                "unresolved": 2,
                "unresolvedCurrent": 1,
                "unresolvedOutdated": 1,
            },
        )
        self.assertEqual(
            [item["id"] for item in report["unresolvedThreads"]],
            ["current", "outdated"],
        )
        self.assertNotIn("after=page-2", calls[0])
        self.assertEqual(calls[1][calls[1].index("after=page-2") - 1], "-f")

    def test_rejects_missing_pagination_cursor(self) -> None:
        with self.assertRaisesRegex(RuntimeError, "without an end cursor"):
            pr_thread_status.collect("owner/repo", 3, lambda _: page([], has_next=True, cursor=""))

    def test_rejects_head_change_during_pagination(self) -> None:
        responses = iter(
            [
                page([], head="head-1", has_next=True, cursor="page-2"),
                page([], head="head-2"),
            ]
        )
        with self.assertRaisesRegex(RuntimeError, "head moved during pagination"):
            pr_thread_status.collect("owner/repo", 3, lambda _: next(responses))

    def test_rejects_missing_pull_request(self) -> None:
        response = json.dumps({"data": {"repository": {"pullRequest": None}}})
        with self.assertRaisesRegex(RuntimeError, "was not found"):
            pr_thread_status.collect("owner/repo", 404, lambda _: response)


class ProcessTest(unittest.TestCase):
    def test_missing_gh_uses_runtime_error_contract(self) -> None:
        with mock.patch.object(
            pr_thread_status.subprocess,
            "run",
            side_effect=FileNotFoundError("missing"),
        ):
            with self.assertRaisesRegex(RuntimeError, "gh CLI not available"):
                pr_thread_status.run_gh(["api", "graphql"])

    def test_nonzero_gh_exit_uses_runtime_error_contract(self) -> None:
        completed = subprocess.CompletedProcess(
            ["gh", "api"], 1, stdout="", stderr="permission denied"
        )
        with mock.patch.object(pr_thread_status.subprocess, "run", return_value=completed):
            with self.assertRaisesRegex(RuntimeError, "permission denied"):
                pr_thread_status.run_gh(["api", "graphql"])

    def test_main_reports_query_failure_as_exit_two(self) -> None:
        stderr = io.StringIO()

        def runner(_: list[str]) -> str:
            raise RuntimeError("network unavailable")

        result = pr_thread_status.main(
            ["--repo", "owner/repo", "--pr", "3"],
            runner=runner,
            stdout=io.StringIO(),
            stderr=stderr,
        )

        self.assertEqual(result, 2)
        self.assertEqual(stderr.getvalue(), "error: network unavailable\n")

    def test_main_reports_missing_pull_request_as_exit_two(self) -> None:
        stderr = io.StringIO()
        response = json.dumps({"data": {"repository": {"pullRequest": None}}})

        result = pr_thread_status.main(
            ["--repo", "owner/repo", "--pr", "404"],
            runner=lambda _: response,
            stdout=io.StringIO(),
            stderr=stderr,
        )

        self.assertEqual(result, 2)
        self.assertEqual(
            stderr.getvalue(),
            "error: pull request owner/repo#404 was not found\n",
        )

    def test_main_reports_nonzero_gh_exit_as_exit_two(self) -> None:
        stderr = io.StringIO()
        completed = subprocess.CompletedProcess(
            ["gh", "api"], 1, stdout="", stderr="permission denied"
        )
        with mock.patch.object(pr_thread_status.subprocess, "run", return_value=completed):
            result = pr_thread_status.main(
                ["--repo", "owner/repo", "--pr", "3"],
                stdout=io.StringIO(),
                stderr=stderr,
            )

        self.assertEqual(result, 2)
        self.assertIn("error: gh api graphql failed: permission denied", stderr.getvalue())


class OutputTest(unittest.TestCase):
    def test_one_line_truncates_to_exact_limit(self) -> None:
        result = pr_thread_status.one_line("x" * 121)
        self.assertEqual(len(result), 120)
        self.assertTrue(result.endswith("…"))

    def test_one_line_replaces_terminal_controls(self) -> None:
        result = pr_thread_status.one_line("before\x1b[2K\x07after")
        self.assertNotIn("\x1b", result)
        self.assertNotIn("\x07", result)
        self.assertEqual(result, "before�[2K�after")

    def test_summary_sanitizes_untrusted_fields(self) -> None:
        report = {
            "repository": "owner/repo",
            "pullRequest": 3,
            "headRefOid": "head-1",
            "counts": {
                "total": 1,
                "resolved": 0,
                "unresolved": 1,
                "unresolvedCurrent": 1,
                "unresolvedOutdated": 0,
            },
            "unresolvedThreads": [
                {
                    "id": "thread-1",
                    "isResolved": False,
                    "isOutdated": False,
                    "path": "src/\x1b[2Kfile.rs",
                    "line": 9,
                    "originalLine": 8,
                    "latestComment": {
                        "author": {"login": "bad\x07actor"},
                        "url": "https://example.test/thread-1",
                        "body": "erase\x1b[1Aline",
                    },
                }
            ],
        }
        output = io.StringIO()

        pr_thread_status.print_summary(report, output)

        self.assertNotIn("\x1b", output.getvalue())
        self.assertNotIn("\x07", output.getvalue())
        self.assertIn("unresolved=1", output.getvalue())

    def test_json_output_is_deterministic_and_round_trips(self) -> None:
        response = page([thread("current", resolved=False, outdated=False)])

        def render() -> str:
            output = io.StringIO()
            result = pr_thread_status.main(
                ["--repo", "owner/repo", "--pr", "3", "--json"],
                runner=lambda _: response,
                stdout=output,
                stderr=io.StringIO(),
            )
            self.assertEqual(result, 0)
            json.loads(output.getvalue())
            return output.getvalue()

        self.assertEqual(render(), render())


if __name__ == "__main__":
    unittest.main()
