---
allowed-tools: Agent, Read, Grep, Glob, Bash(gh pr view:*), Bash(gh pr diff:*), Bash(gh pr review:*), mcp__github_inline_comment__create_inline_comment
description: Review a CircuitC pull request across architecture/spec consistency, correctness, Rust quality, and test coverage, then submit one formal verdict
---

Review the given pull request in one complete pass. Fan out to CircuitC's four
review agents, validate their findings, and post one consolidated, high-signal
review.

**Arguments: `$ARGUMENTS`** — parse the first token as the PR URL or number
(`<PR>`). `--comment` enables GitHub writes. CI also supplies
`--metadata <path>` and `--diff <path>` for one immutable context snapshot. When
both paths exist, read them directly and do not re-fetch PR metadata or the diff.
Without `--comment`, print findings only and make no GitHub writes.

Follow these steps precisely:

1. **Skip gate.** Read the supplied metadata, or make one standalone
   `gh pr view <PR>` call. Stop only when the PR is closed, is a draft that was
   not explicitly requested for review, or is both mechanically trivial and
   behavior-neutral. Never skip a change to a public API, IR/schema, compiler or
   backend boundary, diagnostics, canonicalization/determinism, generated format,
   error handling, workflow security, or build/test authority.

2. **Gather context once.** Use the supplied metadata and diff. For a local dry
   run without prepared paths, make exactly one standalone `gh pr view <PR>` and
   one standalone `gh pr diff <PR>` call. Give all reviewers the same snapshot;
   they must not re-fetch the PR. The diff is authoritative.

3. **Complete all four review dimensions in the foreground.** CI disables
   background tasks. Dispatch these agents with the shared metadata and diff
   paths and ask for only high-confidence findings with file:line, failure case,
   concrete fix, confidence, and severity:

   - **spec-consistency-reviewer** — architecture, epic, ADR, schema, authority,
     exactness, determinism, and backend-contract drift;
   - **correctness-reviewer** — real logic, validation, diagnostic, identity,
     ordering, arithmetic, and error-handling defects;
   - **rust-quality-reviewer** — Rust safety/API design and material compiler
     performance problems not caught by Clippy;
   - **test-coverage-reviewer** — changed behavior missing the required unit,
     golden, repeat-build, process, or host-authority evidence.

   Do not consolidate while any dimension remains incomplete. If an agent cannot
   run or returns nothing usable, perform that dimension yourself from the shared
   snapshot.

4. **Validate directly.** Check every candidate against the shared diff and the
   relevant repository files. It must be real, in scope, and anchored to changed
   code. Drop duplicates, medium-or-lower confidence findings, speculative future
   work, and anything that depends on an unverified assumption. Do not spawn a
   second validation wave.

5. **Classify severity.** A finding is **blocking** when it is a correctness or
   security defect, breaks a written invariant/contract, silently weakens
   semantics, leaves changed behavior untested, or swallows a material error.
   API polish, simplification, traceability, and non-material performance notes
   are **advisory**. Advisory-only reviews approve.

6. **Consolidate and report.** Keep at most one comment per unique issue. Prepare
   a short summary split into Must fix and Advisory, or state that no high-signal
   issues survived across all four dimensions.

7. **Write only when enabled.** Without `--comment`, stop after printing the
   summary. With `--comment`:

   - Post one inline comment per finding using
     `mcp__github_inline_comment__create_inline_comment` with `confirmed: true`.
     Prefix blockers with `[spec]`, `[bug]`, `[rust]`, or `[tests]`; prefix
     advisory findings with `[nit]`. Cite the governing rule for spec findings
     and include a concrete fix.
   - Submit exactly one formal verdict:
     `gh pr review <PR> --request-changes --body "<summary>"` when a blocking
     finding survives, otherwise
     `gh pr review <PR> --approve --body "<summary>"`.

   The body is a concise `## Claude review` section with the verdict, Must fix
   list (or `none`), and advisory count. Never finish with a COMMENTED-only
   review. Zero findings still require one approving formal verdict.

8. **Completion check.** Before finishing, verify that all four dimensions were
   completed, every posted finding was directly validated and deduplicated, every
   inline comment succeeded, and exactly one formal verdict command succeeded.
   Continue or fail explicitly if any item is incomplete.

False positives erode trust. When a finding cannot be justified from the diff and
a concrete CircuitC contract or failure scenario, drop it.
