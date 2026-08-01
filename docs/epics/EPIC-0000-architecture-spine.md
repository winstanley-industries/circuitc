# EPIC-0000: Executable architecture spine

- Status: Complete
- Architecture milestones: M0 and M0.1
- Depends on: none

## Outcome

CircuitC has a headless Rust compiler built exclusively through Bazel, a
validated canonical Design IR, and deterministic KiCad 10 PCB and SPICE
backends. The code-authored voltage divider proves the complete path from
canonical design to host-validated artifacts.

## Requirements

| ID | Requirement |
| --- | --- |
| `CC-REQ-FOUND-001` | CircuitC's compiler core is Rust 2024 and Bazel is its exclusive top-level build, test, lint, and execution interface. |
| `CC-REQ-FOUND-002` | The canonical Design IR represents coordinates and electrical quantities exactly and rejects invalid public values with machine-readable diagnostics rather than panics. |
| `CC-REQ-FOUND-003` | Semantic identities are stable, domain-separated, independent of iteration order and geometry where identity is explicit, and checked for emitted KiCad UUID collisions. |
| `CC-REQ-FOUND-004` | KiCad 10 PCB output is byte-deterministic and accepted by the supported KiCad parser and structured DRC policy. |
| `CC-REQ-FOUND-005` | SPICE output is deterministic, preserves distinct canonical identities through a reversible backend-name map, and runs through the supported Ohmnivore subset. |
| `CC-REQ-FOUND-006` | Logical-pin-to-pad bindings, ground cardinality, route identity, route geometry, and coordinate envelopes are explicitly validated. |
| `CC-REQ-FOUND-007` | Bazel module resolution is pinned and both action-level and full-module-graph strict lockfile gates pass. |

## Non-goals

- A user-facing CircuitC language.
- KiCad schematic or project generation.
- Direct Ohmnivore or APGAR library integration.
- Production library, ERC, DRC, or manufacturing coverage.

## Acceptance evidence

- Commit `dfc15af` contains the completed architecture spine and compiler
  boundary closure.
- `bazel build //...` passed.
- `bazel test //...` passed.
- `bazel test --lockfile_mode=error //...` passed.
- `bazel mod graph --lockfile_mode=error` passed.
- `bazel test //:kicad10_drc_test` passed with KiCad 10.0.5, zero
  unconnected items, and only the two narrowly allowlisted bootstrap library
  warnings.
- Two independent voltage-divider builds produced byte-identical KiCad and
  SPICE files.
- The generated SPICE ran through the Ohmnivore CPU solver with
  `V(VOUT) = 4.999999975 V`.

## Deferred work

The code-authored fixture remains a bootstrap frontend. EPIC-0001 replaces it
as the primary authoring path without weakening any completed backend gate.
