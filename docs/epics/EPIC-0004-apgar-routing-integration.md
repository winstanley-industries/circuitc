# EPIC-0004: APGAR routing integration

- Status: Planned
- Architecture milestone: M3
- Depends on: EPIC-0002

## Outcome

CircuitC lowers exact placed-board intent into a versioned APGAR request,
receives deterministic route candidates with provenance, imports a selected
route without identity loss, and requires both APGAR exact validation and
KiCad DRC before accepting it.

## Requirements

| ID | Requirement |
| --- | --- |
| `CC-REQ-ROUTE-001` | The CircuitC-to-APGAR request and result contracts are versioned, checksummed, CAD-neutral, and independent of APGAR's internal C++ or CUDA layouts. |
| `CC-REQ-ROUTE-002` | Coordinate, layer, connectivity, obstacle, and rule lowering is exact and uses checked conversion between CircuitC nanometres and APGAR database units. |
| `CC-REQ-ROUTE-003` | Route candidates carry stable identity, request identity, toolchain identity, cost data, validation status, and replay provenance. |
| `CC-REQ-ROUTE-004` | A deterministic CPU reference path establishes correctness before GPU performance work is accepted. |
| `CC-REQ-ROUTE-005` | Selected routes import into canonical physical intent and deterministic KiCad output without silently weakening unsupported geometry or rules. |
| `CC-REQ-ROUTE-006` | APGAR exact validation and supported KiCad DRC both gate every accepted imported route. |
| `CC-REQ-ROUTE-007` | Failed, unsupported, stale, or mismatched requests and results produce machine-readable diagnostics and cannot be imported. |

## Non-goals

- Reimplementing APGAR inside CircuitC.
- Sharing mutable in-process implementation objects across the contract.
- GPU optimization before deterministic CPU correctness and replay exist.
- Treating APGAR validation as a substitute for KiCad DRC.

## Acceptance gates

- A checked-in deterministic CPU fixture round-trips through request, route,
  import, APGAR validation, KiCad emission, and KiCad DRC.
- Repeated requests and CPU results are byte-identical.
- Contract corruption, version mismatch, coordinate overflow, unsupported
  rules, and stale provenance are rejected by exact diagnostics.
- GPU results, when enabled, reproduce valid CPU-reference semantics within the
  declared routing contract.
- All repository Bazel and strict lockfile gates pass.

## Completion evidence

Not yet complete.
