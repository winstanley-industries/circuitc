---
allowed-tools: Agent, Read, Grep, Glob, Bash(gh pr view:*), Bash(gh pr diff:*), Bash(gh pr review:*), mcp__github_inline_comment__create_inline_comment
description: Review a CircuitC pull request across architecture/spec consistency, correctness, Rust quality, and test coverage, then submit one formal verdict
---

Review the given pull request in one bounded pass. Use CircuitC's applicable
review agents, validate their findings, and post one consolidated, high-signal
review.

**Arguments: `$ARGUMENTS`** — parse the first token as the PR URL or number
(`<PR>`). `--comment` enables GitHub writes. CI also supplies
`--metadata <path>`, `--diff <path>`, `--history <path>`, `--delta <path>`,
`--mode <initial|follow-up|convergence>`, and `--round <number>` for one
immutable context snapshot. Read those files directly and do not re-fetch PR
metadata, the diff, or review history. Without `--comment`, print findings only
and make no GitHub writes.

Follow these steps precisely:

1. **Skip gate.** Read the supplied metadata, or make one standalone
   `gh pr view <PR>` call. Stop only when the PR is closed, is a draft that was
   not explicitly requested for review, or is both mechanically trivial and
   behavior-neutral. Never skip a change to a public API, IR/schema, compiler or
   backend boundary, diagnostics, canonicalization/determinism, generated format,
   error handling, workflow security, or build/test authority.

2. **Gather context once.** Use the supplied metadata, full PR diff, formal
   review history, and last-reviewed-head delta. For a local dry run without
   prepared paths, make exactly one standalone `gh pr view <PR>` and one
   standalone `gh pr diff <PR>` call and treat it as an initial review. Give all
   reviewers the same snapshot; they must not re-fetch the PR. The full diff is
   authoritative for the PR's current state; the delta bounds new discovery on
   follow-up reviews.

3. **Set the review scope before dispatch.** Apply the supplied mode:

   - **initial** — examine the full PR;
   - **follow-up** — validate prior Must fix items and examine the delta since
     the last formally reviewed head. Use the full diff only as context, not as
     a fresh discovery surface; and
   - **convergence** — after two change-request rounds, examine only unresolved
     prior blockers, regressions introduced by their fixes, and material
     correctness, contract, or security defects in the delta. Route a new
     independent subsystem-level concern to manual review or a split rather than
     starting another hardening cycle. Never suppress or approve with a verified
     blocker.

   If a follow-up or convergence delta expands scope beyond the seams needed to
   address prior Must fix items, review that new scope as an initial pass across
   all applicable dimensions. Do not reopen unchanged earlier code, and request a
   split when the expansion is material.

   Classify changed artifacts and keep evidence proportional to their blast
   radius:

   - product/compiler/backend code receives all applicable contract,
     correctness, Rust, and coverage review;
   - workflow/build/release code receives correctness and security review of
     permissions, untrusted inputs, secrets, external writes, event logic, and
     failure propagation. The correctness reviewer owns this dimension and may
     not mark it not applicable. Existing syntax and security gates are normally
     adequate for purely declarative workflow changes;
   - documentation receives contract and accuracy review. Check executable
     examples that define a required procedure, but do not demand runtime tests
     for prose; and
   - temporary internal tooling receives review proportional to its declared
     support, external effects, and blast radius. Do not impose production API,
     packaging, persistence, adversarial-input, or mutation-complete standards
     merely because a human or agent consults it. It is a protected gate only
     when CI or branch policy actually enforces it.

4. **Complete applicable review dimensions in the foreground.** CI disables
   background tasks. Dispatch only applicable agents with the shared snapshot
   paths and ask for high-confidence findings with file:line, supported failure
   case, material consequence, smallest proportionate fix, confidence, and
   severity:

   - **spec-consistency-reviewer** — architecture, epic, ADR, schema, authority,
     exactness, determinism, and backend-contract drift;
   - **correctness-reviewer** — product logic, validation, diagnostics, identity,
     ordering, arithmetic, error handling, and protected-workflow security;
   - **rust-quality-reviewer** — Rust safety/API design and material compiler
     performance problems not caught by Clippy;
   - **test-coverage-reviewer** — changed behavior missing the required unit,
     golden, repeat-build, process, or host-authority evidence.

   A dimension marked not applicable counts as completed. Do not consolidate
   while an applicable dimension remains incomplete. If an applicable agent
   cannot run or returns nothing usable, perform that dimension yourself from
   the shared snapshot.

5. **Validate directly.** Check every candidate against the shared diff and the
   relevant repository files. It must be real, in scope, and anchored to changed
   code. A blocker must name a trigger from supported operation or an exposed
   attacker-controlled boundary, an observable and material consequence, and the
   governing contract when one applies. Drop duplicates, medium-or-lower
   confidence findings, speculative future work, synthetic faults that cannot
   arise from supported operation or attacker input, and anything that depends on
   an unverified assumption. Prefer the smallest fix; do not turn a narrow defect
   into a new subsystem. Do not spawn a second validation wave.

6. **Classify severity.** A finding is **blocking** only when the current PR
   demonstrably introduces a material supported-use correctness defect,
   exploitable security defect, violation of a written contract, semantic
   weakening, loss of a required diagnostic, or fail-open behavior in a
   protected build/release gate.

   Missing coverage is blocking only when the evidence is required by a written
   contract or protects materially changed product behavior or protected
   automation, and no existing gate exercises that behavior. A surviving
   hypothetical mutation, extra defensive branch, unsupported environment, or
   possible hardening improvement is not blocking by itself.

   API polish, additional defensive validation, optional portability,
   traceability, extra tests beyond the proportionate evidence surface, and
   non-material performance work are advisory. Advisory-only reviews approve and
   request no current-head change.

7. **Consolidate and report.** Keep at most one comment per unique issue. A
   change-request summary contains Must fix items only. An approving summary may
   contain at most three `Optional follow-ups — no current-head change requested`,
   or state that no high-signal issues survived across the applicable dimensions.
   On follow-up and convergence reviews, omit new advisories unless the delta
   introduced them.

8. **Write only when enabled.** Without `--comment`, stop after printing the
   summary. With `--comment`:

   - Post inline comments only for blocking findings using
     `mcp__github_inline_comment__create_inline_comment` with `confirmed: true`.
     Prefix blockers with `[spec]`, `[bug]`, `[rust]`, or `[tests]`. Cite the
     governing rule for spec findings and include the smallest proportionate fix.
     Put optional follow-ups only in the approving review body; do not create
     advisory review threads.
   - Submit exactly one formal verdict:
     `gh pr review <PR> --request-changes --body "<summary>"` when a blocking
     finding survives, otherwise
     `gh pr review <PR> --approve --body "<summary>"`.

   The body is a concise `## Claude review` section with the verdict and Must fix
   list, or an approving verdict plus optional follow-ups. End the body with
   exactly one machine-readable marker:
   `<!-- circuitc-claude-verdict: request-changes -->` for a change request or
   `<!-- circuitc-claude-verdict: approve -->` for an approval. Never finish with
   a COMMENTED-only review. Zero findings still require one approving formal
   verdict and its marker.

9. **Completion check.** Before finishing, verify that all applicable dimensions
   were completed or marked not applicable, every posted blocker was directly
   validated and deduplicated, every inline comment succeeded, and exactly one
   formal verdict command succeeded. Continue or fail explicitly if any item is
   incomplete.

False positives erode trust. When a finding cannot be justified from the diff and
a concrete CircuitC contract or failure scenario, drop it.
