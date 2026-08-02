#!/usr/bin/env python3
"""Report exact-head GitHub pull-request review-thread state without writes."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from typing import Any

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
    process = subprocess.run(
        ["gh", *arguments],
        capture_output=True,
        check=False,
        text=True,
    )
    if process.returncode != 0:
        detail = process.stderr.strip() or process.stdout.strip()
        raise RuntimeError(f"gh {' '.join(arguments[:2])} failed: {detail}")
    return process.stdout


def current_repo() -> str:
    return run_gh(["repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner"]).strip()


def parse_repo(value: str) -> tuple[str, str]:
    parts = value.strip().split("/")
    if len(parts) != 2 or not all(parts):
        raise ValueError("repository must be OWNER/REPO")
    return parts[0], parts[1]


def graphql_page(owner: str, name: str, number: int, after: str | None) -> dict[str, Any]:
    arguments = [
        "api",
        "graphql",
        "-f",
        f"query={QUERY}",
        "-F",
        f"owner={owner}",
        "-F",
        f"name={name}",
        "-F",
        f"number={number}",
    ]
    if after is not None:
        arguments.extend(["-F", f"after={after}"])
    return json.loads(run_gh(arguments))


def collect(repo: str, number: int) -> dict[str, Any]:
    owner, name = parse_repo(repo)
    after: str | None = None
    threads: list[dict[str, Any]] = []
    head_oid: str | None = None

    while True:
        payload = graphql_page(owner, name, number, after)
        pull_request = payload.get("data", {}).get("repository", {}).get("pullRequest")
        if pull_request is None:
            raise RuntimeError(f"pull request {repo}#{number} was not found")
        head_oid = pull_request["headRefOid"]
        connection = pull_request["reviewThreads"]
        threads.extend(connection["nodes"])
        page_info = connection["pageInfo"]
        if not page_info["hasNextPage"]:
            break
        after = page_info["endCursor"]
        if not after:
            raise RuntimeError("GitHub reported another page without an end cursor")

    normalized = []
    for thread in threads:
        latest_nodes = thread.pop("comments", {}).get("nodes", [])
        latest = latest_nodes[-1] if latest_nodes else None
        normalized.append({**thread, "latestComment": latest})

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
    compact = " ".join(value.split())
    return compact if len(compact) <= limit else compact[: limit - 1] + "…"


def print_summary(report: dict[str, Any]) -> None:
    counts = report["counts"]
    print(f"{report['repository']}#{report['pullRequest']} head {report['headRefOid']}")
    print(
        "threads "
        f"total={counts['total']} resolved={counts['resolved']} "
        f"unresolved={counts['unresolved']} current={counts['unresolvedCurrent']} "
        f"outdated={counts['unresolvedOutdated']}"
    )
    for thread in report["unresolvedThreads"]:
        location = thread.get("line") or thread.get("originalLine") or "?"
        state = "outdated" if thread["isOutdated"] else "current"
        latest = thread.get("latestComment") or {}
        author = (latest.get("author") or {}).get("login") or "unknown"
        print(
            f"- {thread['id']} [{state}] {thread.get('path') or '?'}:{location} "
            f"@{author} {latest.get('url') or ''} {one_line(latest.get('body'))}"
        )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--repo", help="GitHub repository as OWNER/REPO; defaults to current gh repo"
    )
    parser.add_argument("--pr", required=True, type=int, help="pull-request number")
    parser.add_argument("--json", action="store_true", help="emit machine-readable JSON")
    args = parser.parse_args()

    try:
        repo = args.repo or current_repo()
        report = collect(repo, args.pr)
    except (RuntimeError, ValueError, json.JSONDecodeError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2

    if args.json:
        json.dump(report, sys.stdout, indent=2, sort_keys=True)
        sys.stdout.write("\n")
    else:
        print_summary(report)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
