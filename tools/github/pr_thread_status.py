"""Report exact-head GitHub pull-request review-thread state without writes."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from collections.abc import Callable, Sequence
from typing import Any, TextIO

Runner = Callable[[list[str]], str]

_CONTROL = re.compile(r"[\x00-\x08\x0b-\x1f\x7f-\x9f]")
_GH_TIMEOUT_SECONDS = 30

QUERY = r"""
query ReviewThreads(
  $owner: String!
  $name: String!
  $number: Int!
  $after: String
) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      headRefOid
      reviewThreads(first: 100, after: $after) {
        pageInfo { hasNextPage endCursor }
        nodes {
          id
          isResolved
          isOutdated
          path
          line
          originalLine
          comments(last: 1) {
            nodes {
              author { login }
              body
              url
              createdAt
            }
          }
        }
      }
    }
  }
}
"""


def run_gh(arguments: list[str]) -> str:
    try:
        process = subprocess.run(
            ["gh", *arguments],
            capture_output=True,
            check=False,
            text=True,
            timeout=_GH_TIMEOUT_SECONDS,
        )
    except subprocess.TimeoutExpired as error:
        raise RuntimeError(f"gh command timed out after {_GH_TIMEOUT_SECONDS} seconds") from error
    except OSError as error:
        raise RuntimeError(f"gh CLI not available: {error}") from error
    if process.returncode != 0:
        detail = process.stderr.strip() or process.stdout.strip()
        raise RuntimeError(f"gh {' '.join(arguments[:2])} failed: {detail}")
    return process.stdout


def parse_repo(value: str) -> tuple[str, str]:
    parts = value.strip().split("/")
    if len(parts) != 2 or not all(parts):
        raise ValueError("repository must be OWNER/REPO")
    return parts[0], parts[1]


def graphql_page(
    owner: str,
    name: str,
    number: int,
    after: str | None,
    runner: Runner = run_gh,
) -> dict[str, Any]:
    arguments = [
        "api",
        "graphql",
        "-f",
        f"query={QUERY}",
        "-f",
        f"owner={owner}",
        "-f",
        f"name={name}",
        "-F",
        f"number={number}",
    ]
    if after is not None:
        arguments.extend(["-f", f"after={after}"])
    return json.loads(runner(arguments))


def collect(repo: str, number: int, runner: Runner = run_gh) -> dict[str, Any]:
    owner, name = parse_repo(repo)
    after: str | None = None
    threads: list[dict[str, Any]] = []
    head_oid: str | None = None

    while True:
        payload = graphql_page(owner, name, number, after, runner)
        errors = payload.get("errors") or []
        if errors:
            raise RuntimeError(f"GitHub GraphQL errors: {json.dumps(errors, sort_keys=True)}")
        pull_request = payload.get("data", {}).get("repository", {}).get("pullRequest")
        if pull_request is None:
            raise RuntimeError(f"pull request {repo}#{number} was not found")

        page_head = pull_request.get("headRefOid")
        if not isinstance(page_head, str) or not page_head:
            raise RuntimeError("GitHub response omitted the pull request head OID")
        if head_oid is None:
            head_oid = page_head
        elif head_oid != page_head:
            raise RuntimeError(f"head moved during pagination: {head_oid} -> {page_head}")

        connection = pull_request["reviewThreads"]
        threads.extend(connection.get("nodes") or [])
        page_info = connection["pageInfo"]
        if not page_info["hasNextPage"]:
            break
        after = page_info["endCursor"]
        if not after:
            raise RuntimeError("GitHub reported another page without an end cursor")

    normalized = []
    for thread in threads:
        latest_nodes = thread.get("comments", {}).get("nodes", [])
        latest = latest_nodes[-1] if latest_nodes else None
        thread_fields = {key: value for key, value in thread.items() if key != "comments"}
        normalized.append({**thread_fields, "latestComment": latest})

    unresolved = [thread for thread in normalized if not thread["isResolved"]]
    return {
        "repository": repo,
        "pullRequest": number,
        "headRefOid": head_oid,
        "counts": {
            "total": len(normalized),
            "resolved": len(normalized) - len(unresolved),
            "unresolved": len(unresolved),
            "unresolvedCurrent": sum(not thread["isOutdated"] for thread in unresolved),
            "unresolvedOutdated": sum(thread["isOutdated"] for thread in unresolved),
        },
        "unresolvedThreads": unresolved,
    }


def one_line(value: str | None, limit: int = 120) -> str:
    if not value:
        return ""
    compact = _CONTROL.sub("�", " ".join(value.split()))
    return compact if len(compact) <= limit else compact[: limit - 1] + "…"


def print_summary(report: dict[str, Any], stream: TextIO = sys.stdout) -> None:
    counts = report["counts"]
    print(
        f"{one_line(report['repository'])}#{report['pullRequest']} "
        f"head {one_line(report['headRefOid'])}",
        file=stream,
    )
    print(
        "threads "
        f"total={counts['total']} resolved={counts['resolved']} "
        f"unresolved={counts['unresolved']} current={counts['unresolvedCurrent']} "
        f"outdated={counts['unresolvedOutdated']}",
        file=stream,
    )
    for thread in report["unresolvedThreads"]:
        location = thread.get("line") or thread.get("originalLine") or "?"
        state = "outdated" if thread["isOutdated"] else "current"
        latest = thread.get("latestComment") or {}
        author = (latest.get("author") or {}).get("login") or "unknown"
        print(
            f"- {one_line(thread['id'])} [{state}] "
            f"{one_line(thread.get('path') or '?', 240)}:{location} "
            f"@{one_line(author)} {one_line(latest.get('url') or '', 240)} "
            f"{one_line(latest.get('body'))}",
            file=stream,
        )


def main(
    argv: Sequence[str] | None = None,
    *,
    runner: Runner = run_gh,
    stdout: TextIO = sys.stdout,
    stderr: TextIO = sys.stderr,
) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", required=True, help="GitHub repository as OWNER/REPO")
    parser.add_argument("--pr", required=True, type=int, help="pull-request number")
    parser.add_argument("--json", action="store_true", help="emit machine-readable JSON")
    args = parser.parse_args(argv)

    try:
        report = collect(args.repo, args.pr, runner)
    except (RuntimeError, ValueError, json.JSONDecodeError) as error:
        print(f"error: {error}", file=stderr)
        return 2

    if args.json:
        json.dump(report, stdout, indent=2, sort_keys=True)
        stdout.write("\n")
    else:
        print_summary(report, stdout)
    return 0
