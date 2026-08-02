# CircuitC epic delivery ledger

Create this ledger before broad implementation. Keep it in `.agent-scratch/` unless the epic itself should own a durable summary.

## Epic acceptance header

```markdown
# EPIC-NNNN delivery ledger

- Base: origin/main at <oid>
- Owning epic: docs/epics/<file>
- Governing ADRs: <paths>
- Affected schemas: <paths>
- Terminal outcome: local | PRs open | merged | epic complete
- GitHub write authority: open PR <yes/no>; reply <yes/no>; resolve <yes/no>; follow-up issue <yes/no>; merge <yes/no>; delete remote branch <yes/no>
- Supported host/tool versions: <versions>
```

## Stack plan

```markdown
| Layer | Contract claim | Requirements | Base | Owned paths | Independent gates | State |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | <one claim> | <IDs> | origin/main | <paths> | <commands> | planned |
| 2 | <one claim> | <IDs> | layer-1 | <paths> | <commands> | planned |
```

For every layer, answer:

- Can it merge while leaving the repository correct and useful?
- Can a reviewer validate it without mentally reviewing later layers?
- Does its diff contain only its immediate dependency delta?
- Does it have one central contract owner?
- Are its tests discriminating without relying on later layers?

If any answer is no, revise the boundary.

## Risk and coverage matrix

```markdown
| Row | Requirement/contract | Changed behavior or failure path | Layer | Implementation seam | Success test | Failure mutant and expected failing test | Determinism evidence | Host evidence | Likely review attack | Status |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| R1 | CC-REQ-... | <behavior> | 1 | <path:symbol> | <target/assertion> | remove <guard>; <test> fails | repeat/cmp | n/a | <omission> | open |
```

Add distinct rows for:

- every new stable diagnostic code, message ownership, and source span;
- every emitted schema field or KiCad/SPICE stanza;
- ordering, canonicalization, identity, and collision rules;
- exact arithmetic and conversion boundaries;
- unsupported inputs and partial/malformed artifacts;
- filesystem containment, replacement, interruption, and synchronization;
- every structured host-report finding category and allowlist decision;
- CLI/FFI/process exit behavior;
- clean-checkout, offline, and user-global-configuration isolation;
- public APIs and authored paths that can bypass a lower-level guard.

Rows are complete only when deleting or corrupting the named behavior makes the stated test fail.

## Pre-PR adversarial review log

```markdown
| Dimension | Reviewer | Snapshot base..head | Findings | Verified blockers fixed | Recheck |
| --- | --- | --- | --- | --- | --- |
| Spec/contracts | <agent> | <oids> | <IDs/none> | <commit/n-a> | pass |
| Correctness | <agent> | <oids> | <IDs/none> | <commit/n-a> | pass |
| Security/transactions | <agent> | <oids> | <IDs/none> | <commit/n-a> | pass |
| Rust/performance | <agent> | <oids> | <IDs/none> | <commit/n-a> | pass |
| Mutation/coverage | <agent> | <oids> | <IDs/none> | <commit/n-a> | pass |
| Host/evidence | <agent> | <oids> | <IDs/none> | <commit/n-a> | pass |
```

## Feedback round log

```markdown
| Round | Head | Thread cluster | Classification | Ledger rows | Fix/follow-up | Gates | Exact-head review result | Threads resolved |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
```

Before a second fix round, ask whether the new blockers belong to a different contract owner than the first round. If yes, split the remaining work instead of expanding the current PR again.

## Exact-head closeout

```markdown
- PR: <url>
- Head OID: <oid>
- Immediate base: <branch/oid>
- Required checks: <names/results>
- Automated verdict on head: <state/review id>
- Human verdict on head: <state/review id>
- Unresolved current threads: 0
- Unresolved outdated threads: 0
- Local gates: <commands/results>
- Host gate: <command/version/result or unavailable>
- Merge method and expected-head protection: <method/oid>
- Remote merge result: <merge oid>
- origin/main verification: <result>
```
