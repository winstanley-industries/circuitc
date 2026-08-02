---
name: circuitc-epic-implementation
description: Implement and ship CircuitC epics end to end, from repository-grounded design and contract-based stacked PR decomposition through implementation, Bazel and KiCad validation, multi-agent adversarial review, GitHub publication, exact-head CI and automated-review monitoring, feedback integration, review-thread replies and resolution, merge, completion audit, and retrospective. Use for CircuitC epic planning, implementation, PR closeout, or any request to carry a substantial CircuitC change through review and merge.
---

# CircuitC Epic Implementation

## Purpose

Carry a CircuitC epic to a verified merged outcome without turning one integrated acceptance plan into one monolithic review surface. Preserve CircuitC's compiler contracts, land work in dependency-ordered PRs, and treat feedback integration and review-thread cleanup as part of delivery.

## Operating contract

- Continue through the lifecycle endpoint the user requested. Do not stop at local implementation when publication, monitoring, merge, or retrospective is in scope.
- Distinguish planned, locally validated, CI-validated, host-validated, approved, and merged states. Bind every claim to an exact commit or PR head.
- Preserve unrelated worktree changes. Stage explicit paths and keep temporary probes under `.agent-scratch/`.
- Ask for new authority before crossing an unrequested external-write boundary. At kickoff, establish whether the request authorizes opening PRs, posting replies, resolving addressed threads, creating follow-up issues, merging, and deleting remote branches.
- Never resolve an unaddressed thread, dismiss a valid review, bypass branch protection, or claim an unexecuted gate passed.
- Keep the user informed at least once per minute while watching long checks or reviews.

## 1. Establish repository truth

1. Locate the CircuitC checkout and read, in this order:
   - `AGENTS.md`;
   - `docs/architecture.md`;
   - the owning file under `docs/epics/`;
   - every accepted ADR touching the change;
   - every affected versioned contract under `schemas/`;
   - relevant language, backend, and host-validation documentation.
2. Inspect `git status`, current branch, remotes, `origin/main`, open PRs, installed `gh` extensions, repository merge settings, and branch protection. Fetch current remote state before planning publication.
3. Reconcile the epic with current code. Do not assume a Planned epic is unimplemented or a Complete epic is fully integrated.
4. Record the requested terminal outcome and GitHub-write authority. If the user asks to implement and ship an epic, publication and monitoring are in scope; merging, thread resolution, follow-up issues, and remote-branch deletion still require clear authorization in the request or a concise confirmation.
5. Maintain a visible plan spanning decomposition, implementation, gates, adversarial review, PR closeout, merge, and retrospective.

## 2. Convert the epic into an acceptance plan and PR stack

Read [references/delivery-ledger.md](references/delivery-ledger.md) and create the stack plan and risk/coverage ledger before broad coding.

Split by authority and review contract, not by a fixed file count. Start with these candidate boundaries and combine only when the result remains independently reviewable:

1. source language, Design IR, schemas, and semantic validation;
2. vendored catalog and backend assets;
3. deterministic KiCad, SPICE, or APGAR lowering;
4. identity maps, diagnostics, and host-report normalization;
5. transactional CLI, FFI, or artifact publication;
6. supported-host acceptance, integration evidence, and completion docs.

For each proposed PR, require one coherent claim, one named contract owner, explicit requirements, independent tests, and a mergeable intermediate repository state. File count is only a smell: a five-file change can span three contracts, while a larger generated catalog change may still be one reviewable unit.

Use a stacked-PR extension when installed. Discover its exact command and syntax with `gh extension list` and its help output; do not guess. Otherwise create ordinary dependency branches. Each upper PR targets its immediate predecessor. Open upper layers as drafts and normally mark only the lowest unmerged layer ready for review. Merge or rebase bottom-up, retargeting the next layer after its dependency lands.

### Mandatory split triggers

Split or restack before opening review when any condition holds:

- the slice changes three or more independent authority boundaries;
- a reviewer must understand unrelated subsystems to validate the central claim;
- the coverage ledger cannot name a focused test command and failure mutant for each behavior;
- the diff combines schema evolution, backend emission, transactional publication, and host evidence without separable commits;
- two feedback rounds expose blocking defects in different previously untracked subsystems;
- an approved head would need broad advisory cleanup that is not required for correctness or the written contract.

When a split trigger appears after publication, stop the loop. Preserve already-reviewed work, extract the remaining contract into a new stacked PR, and explain the revised dependency graph.

## 3. Build the risk and coverage ledger

Create one row for every epic requirement and every changed public behavior, diagnostic, emitted field or stanza, failure branch, identity rule, ordering rule, and host-policy decision. Each row must contain:

- governing contract and owning PR;
- implementation seam;
- success evidence;
- a test that would fail if the new behavior or guard were removed;
- determinism or repeat-build evidence where relevant;
- host-authority evidence where relevant;
- unsupported or failure behavior and stable diagnostic;
- expected automated-review attack or likely omission.

Treat a missing row or a row without discriminating evidence as a blocker before broad implementation. Exercise mutations mentally or with focused tests: remove the guard, reorder declarations, duplicate identities, overflow exact arithmetic, corrupt a manifest, interrupt publication, provide an unknown binding, or inject an ERC/DRC finding. A default full-suite pass is not proof if the behavior can be deleted without a failure.

## 4. Execute one stack layer at a time

1. Sync the base layer to current `origin/main`; base each upper layer on the exact predecessor head.
2. Use subagents for independently bounded work. Give each agent one contract dimension, file ownership, requirements, non-goals, validation commands, and a no-overlap rule. Keep the primary agent responsible for integration and all repository-wide claims.
3. Implement a vertical slice with observable inputs and outputs. Avoid empty subsystem scaffolding.
4. Preserve these non-negotiable contracts:
   - source and canonical Design IR remain authoritative;
   - coordinates stay exact signed integer nanometres until backend conversion;
   - electrical quantities stay exact decimals with dimensions until simulator lowering;
   - identities and output order derive only from stable semantics;
   - unsupported input fails with machine-readable diagnostics;
   - KiCad parser, structured ERC, and structured DRC remain final KiCad authorities;
   - APGAR and Ohmnivore cross only explicit versioned boundaries;
   - CircuitC remains headless and Bazel remains the exclusive top-level interface;
   - the unreleased Design IR evolves in place at schema version 1, with no version bumps, migrations, or compatibility adapters; record semantic changes in `schemas/` and, when authority or wire boundaries move, in an ADR.
5. Update architecture, ADR, epic, and schema documentation whenever implementation intentionally changes their authority, determinism, ownership, or wire semantics.
6. Run focused tests while developing and update the ledger immediately when implementation reveals another behavior or failure path.

## 5. Run required validation

Run the narrowest relevant targets first, then execute the repository gates on the exact candidate head:

```sh
bazel lint //...
bazel build //...
bazel build --lockfile_mode=error //...
bazel test //...
bazel test --lockfile_mode=error //...
bazel mod graph --lockfile_mode=error
```

This is the current Bazel gate set, not the full policy-facing CI surface. Re-read `AGENTS.md`, `README.md`, and `.github/workflows/ci.yml`, then run every additional local gate required for the changed paths. CI runs the pinned workflow-security gate on every pull request; reproduce it locally whenever a file under `.github/workflows/` changes:

```sh
pipx run zizmor==1.25.2 --persona=regular .github/workflows/
```

Report any unavailable gate exactly rather than assuming one invocation is equivalent to another policy signal.

For KiCad backend, artifact, mapping, or policy changes, additionally:

- repeat the build and byte-compare deterministic outputs;
- parse generated artifacts with the supported `kicad-cli` version;
- run the Bazel-owned host gate, normally:

```sh
bazel test //:kicad10_drc_test --nocache_test_results --test_output=errors
```

- inspect normalized structured ERC, DRC, unconnected, connectivity, and parity results rather than trusting exit status;
- state explicitly when a `local` or `manual` host gate is unavailable or is not part of default Linux CI.

Run `git diff --check` and verify generated artifacts contain no host paths, timestamps, unstable ordering, or other nondeterminism. Before publication, run the full matrix on a committed candidate head so every result has a real OID. Any subsequent file change creates a new candidate and invalidates the affected evidence.

## 6. Conduct a pre-PR multi-agent adversarial review

Give every reviewer the same immutable base/head diff and the governing epic, ADRs, schemas, and ledger. Run independent dimensions in parallel:

- architecture/spec/contract consistency;
- correctness, validation, identity, ordering, arithmetic, and error handling;
- security, path handling, FFI, and transactional failure behavior;
- Rust API quality plus material performance or resource risks;
- mutation-oriented test coverage and deterministic artifact evidence;
- supported-host and clean-checkout evidence.

Require each finding to name a concrete failure, file and line, governing contract, severity, confidence, and fix. Use a separate verification pass to reproduce and deduplicate candidates. Fix all verified blockers, update the ledger, rerun affected narrow tests and the full gates, then perform one focused recheck of changed seams. Do not keep launching broad undirected waves after every small fix.

The pre-PR review is complete only when every dimension reports, every blocking candidate is independently verified or rejected with evidence, and the PR is expected to make external review confirmatory rather than exploratory.

## 7. Publish a reviewable layer

Use `github:yeet` when available for focused commit, push, and PR creation. Stage only the intended layer. Before publishing, confirm the branch diff against its immediate base and ensure lower-layer changes are not duplicated in the displayed PR diff.

The PR body must include:

- one-sentence contract claim and owning epic requirements;
- immediate stacked dependency and merge order;
- explicit scope and non-goals;
- implementation/coverage ledger summary;
- exact commands already run and their head OID;
- host gates run, unavailable, or intentionally manual;
- risks, follow-ups, and any historical evidence clearly labeled as historical.

Open as draft until local gates and the adversarial review pass. Mark only the active review layer ready unless parallel review is intentionally safe.

## 8. Watch exact-head CI and automated review

Use `github:gh-fix-ci` for failing Actions checks and `github:gh-address-comments` for thread-aware feedback when available. Treat the following as separate state dimensions:

- current PR head OID;
- draft/ready and mergeability state;
- required policy-facing `pull_request` checks;
- automated-review run and formal verdict on that exact head;
- human review state;
- unresolved current and outdated review threads.

Watch checks through completion, not merely until ordinary CI is green. Manual `workflow_dispatch` success is confidence evidence, not a substitute for required PR-event status. After every push, discard stale successes and approvals and restart the snapshot from the new head.

Use `github:gh-address-comments` to fetch paginated, thread-aware state. If it is unavailable, use the authenticated read-only GraphQL query in [references/github-closeout.md](references/github-closeout.md). Capture the PR head with the thread snapshot, require it to remain stable across pagination, and distinguish unresolved current threads from unresolved outdated threads. Keep inspection separate from reply and resolution mutations.

## 9. Integrate feedback in bounded rounds

1. Fetch all unresolved thread-aware context, not only flat PR comments.
2. Cluster the full set by root cause and ledger row before editing.
3. Classify each item:
   - **blocking and valid:** fix in the current layer;
   - **valid but belongs to another contract:** move it to the correct stacked layer;
   - **advisory:** fix only when it materially improves the current claim without destabilizing an approved head; otherwise record a follow-up;
   - **stale, duplicate, or incorrect:** reply with concrete evidence and make no code change.
4. Make one coherent patch per cluster rather than one commit per comment. Add a regression test that fails without each behavioral fix.
5. Run focused tests, required full gates, and host gates proportionate to the change. Commit and push one validated round.
6. Wait for policy-facing checks and the automated reviewer on the new exact head before declaring the round successful.
7. Apply the mandatory split trigger after two rounds that reveal blockers in different new subsystems.

Once an exact head is approved and green, do not modify it for ordinary nits. Capture those as follow-ups unless they reveal a contract, correctness, security, or coverage defect.

## 10. Reply to and resolve addressed threads immediately

Thread resolution is a distinct delivery operation. After a fix is pushed and the relevant exact-head policy checks and review rerun confirm it:

1. Reply to each addressed thread with the fixing commit and specific validation evidence.
2. Resolve the thread only when the code now satisfies it, the changed code makes it inapplicable, or the reviewer accepted the evidence.
3. For rejected feedback, explain why with a contract or test citation; resolve only when authorized and the disposition is unambiguous.
4. Leave unresolved any partially addressed, disputed, or newly failing issue.
5. Re-fetch the thread-aware snapshot immediately and require zero unresolved threads before merge.

Prefer `github:gh-address-comments`. If the connector cannot expose thread state or resolution, use authenticated `gh api graphql` as described in [references/github-closeout.md](references/github-closeout.md). Keep reply and resolution operations auditable, use small mutation batches, and verify every returned thread state. Do not defer thread cleanup until the merge command fails.

## 11. Enforce exact-head merge readiness

A stack layer is merge-ready only when all are true on one exact head:

- the PR is ready, mergeable, and based on the intended predecessor;
- all required policy-facing checks are green;
- the latest required automated and human verdicts approve that exact head;
- the required local and supported-host gates for that head are recorded;
- the coverage ledger is complete;
- unresolved review-thread count is zero;
- no verified blocker or uncommitted intended change remains.

Inspect allowed merge methods and repository policy. Use the selected method with expected-head protection, for example:

```sh
gh pr merge NUMBER --repo OWNER/REPO --squash --match-head-commit HEAD_OID
```

Add `--delete-branch` only when remote-branch cleanup is authorized. Do not admin-bypass substantive checks or conversation resolution. If a merge command reports an error, re-read remote PR state before retrying; the remote merge may have succeeded even if local branch cleanup failed.

After merge, verify the remote PR state, fetch `origin/main`, confirm the expected commit/tree is reachable, and check the worktree. Then retarget/rebase the next stacked layer and repeat from exact-head validation.

## 12. Complete the epic and conduct the retrospective

After the final layer merges:

1. Audit every epic requirement against merged implementation and exact evidence.
2. Run the integrated clean-checkout and supported-host gates required by the epic.
3. Update epic status and completion evidence only when all dependency PRs are merged. Label point-in-time digests and earlier heads as historical; never imply they prove a later head.
4. Report merged PRs, final main OID, local/CI/host gate results, unresolved follow-ups, and any unavailable evidence.
5. Conduct a concise retrospective covering:
   - whether contract boundaries were correct;
   - review rounds and blocker categories per PR;
   - rework versus CI/reviewer waiting;
   - whether a split trigger fired soon enough;
   - missing ledger rows or host evidence;
   - one durable workflow change, if warranted.
6. Modify repository process documentation or this skill only when the user asks for that durable change. Never update Codex memory without an explicit request.

Finish only when the requested lifecycle endpoint is genuinely reached or a specific authority, external dependency, or unavailable gate is reported as the blocker.
