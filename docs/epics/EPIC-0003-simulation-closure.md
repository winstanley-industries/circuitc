# EPIC-0003: Simulation as a checked compiler phase

- Status: Planned
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

The dependency-ordered implementation stack provides versioned simulation
contracts; typed DC, linear-AC, and transient intent; deterministic lowering;
bounded Bazel-owned Ohmnivore CPU execution; authenticated normalized results;
checked assertions; transactional publication; and the explicit
`//:ngspice45_differential_test` host gate. ADR-0006 records the compared signal
inventory, axis policy, and named numerical tolerances.

The epic remains incomplete until the stack is merged and the final
clean-checkout Bazel, strict-lockfile, exact ngspice 45.2 host, and integration
audit evidence is recorded here.
