# EPIC-0003: Simulation as a checked compiler phase

- Status: Complete
- Architecture milestone: M2
- Depends on: EPIC-0001; selected model bindings may depend on EPIC-0002

## Outcome

CircuitC source declares simulation models, analyses, tolerances, and
assertions. Compilation rejects unsupported model coverage, invokes a selected
simulator reproducibly, maps results back to canonical identities, and fails
when an assertion is not satisfied.

## Requirements

| ID | Requirement |
| --- | --- |
| `CC-REQ-SIM-001` | Simulation model bindings and terminal mappings are explicit, dimensionally checked, and separate from the canonical connectivity model. |
| `CC-REQ-SIM-002` | DC operating-point, AC sweep, and transient analyses have typed source declarations and versioned backend capability checks. |
| `CC-REQ-SIM-003` | Ohmnivore integrates through a Bazel-owned, versioned contract and its solver IR does not become CircuitC's canonical IR. |
| `CC-REQ-SIM-004` | SPICE and result artifacts are deterministic and retain reversible mappings between backend names and CircuitC identities. |
| `CC-REQ-SIM-005` | Numerical assertions carry units, tolerances, analysis context, and explicit pass, fail, unsupported, or unevaluated status. |
| `CC-REQ-SIM-006` | Overlapping device and analysis coverage is differentially tested against ngspice within documented tolerances. |
| `CC-REQ-SIM-007` | Unsupported devices, analyses, convergence states, or result mappings fail with machine-readable diagnostics rather than approximation or omission. |

## Non-goals

- Rewriting Ohmnivore inside CircuitC.
- Treating floating-point simulator state as canonical Design IR.
- Claiming analog signoff coverage beyond declared fixtures and tolerances.
- Board-routing or manufacturing analysis.

## Acceptance gates

- CPU reference fixtures cover every supported device and analysis.
- Ohmnivore and ngspice differential tests pass within named tolerances.
- Assertions deterministically pass or fail the Bazel action as declared.
- Unsupported-capability fixtures produce exact diagnostics.
- Repeated runs produce identical request, mapping, result, and normalized
  report structures where the selected solver is deterministic.
- All repository Bazel and strict lockfile gates pass.

## Completion evidence

The dependency-ordered implementation stack merged through pull requests #6,
#7, #9, #10, #12, and #13. The atomic stack merge produced `main` commit
`36ad7a03a928e55f748a14ffea3c5cbae0cbf28c`; its tree
`f284a4c4351be16f6c353fd3132af8dcc9c301ef` exactly matches the reviewed top
head `c5e871f5faaaaff240385b9ebfdbccbb836ae8f2`. The merged implementation
provides versioned simulation contracts; typed DC, linear-AC, and transient
intent; deterministic lowering; bounded Bazel-owned Ohmnivore CPU execution;
authenticated normalized results; checked assertions; transactional
publication; and the explicit `//:ngspice45_differential_test` host gate.
ADR-0006 records the compared signal inventory, axis policy, and named
numerical tolerances.

A clean detached checkout of that merged `main` commit passed the following
completion gates on macOS arm64 with Bazel 9.2.0:

- `bazel lint //...`
- `bazel build //...`
- `bazel build --lockfile_mode=error //...`
- uncached ordinary and strict-lockfile `bazel test //...` runs, each with all
  12 test targets passing
- uncached `//:ngspice_differential_unit_test`,
  `//:ngspice_differential_process_test`, `//:checked_simulation_cli_test`, and
  `//:ohmnivore_cpu_test`
- `bazel mod graph --lockfile_mode=error`
- a download-disabled strict-lockfile `bazel build //...`
- uncached `//:ngspice45_differential_test` against ngspice 45.2

The first cold local full-test attempt encountered the configured handshake
deadline in two full-runner unit tests while the suite was under concurrent
load. The uncached focused rerun and subsequent complete ordinary and
strict-lockfile matrices passed; no functional mismatch reproduced. GitHub
Actions run `30877166300`, bound to the exact merged commit, independently
passed the Linux and workflow-security jobs. All six merged pull requests have
zero unresolved review conversations.
