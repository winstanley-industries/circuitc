# EPIC-0005: Product and manufacturing closure

- Status: Planned
- Architecture milestone: M4
- Depends on: EPIC-0002; routing-dependent releases also depend on EPIC-0004

## Outcome

CircuitC deterministically produces the product, sourcing, fabrication,
assembly, and release evidence required to manufacture a declared design and
trace every artifact back to source, constraints, and toolchains.

## Requirements

| ID | Requirement |
| --- | --- |
| `CC-REQ-PROD-001` | Parts separate logical function, manufacturer, manufacturer part number, package, lifecycle, sourcing constraints, and approved substitutions. |
| `CC-REQ-PROD-002` | Product variants and fitted, not-fitted, alternate, and configuration states are explicit canonical intent rather than edits to generated outputs. |
| `CC-REQ-PROD-003` | BOM, placement, fabrication, and assembly outputs are deterministic, schema-defined, and cross-checked for identity and quantity consistency. |
| `CC-REQ-PROD-004` | Remote catalog or lifecycle data is captured as pinned evidence and is never required from the network to reproduce a committed release. |
| `CC-REQ-PROD-005` | Board-level signal, power, thermal, or manufacturability analyses integrate as capability-declared adapters with structured results and assertions. |
| `CC-REQ-PROD-006` | A release manifest binds source identity, Design IR identity, generated artifacts, backend versions, validation reports, and checksums. |
| `CC-REQ-PROD-007` | Release generation fails on stale, inconsistent, unsupported, or unvalidated product and manufacturing intent. |

## Non-goals

- Operating a live procurement marketplace.
- Making remote catalog availability a build dependency.
- Replacing specialist fabrication, assembly, or analysis tools.
- Claiming manufacturing readiness without the exact declared validation set.

## Acceptance gates

- A clean checkout reproduces every release artifact and manifest checksum with
  pinned inputs and toolchains.
- BOM, placement, fabrication, and assembly identities reconcile without
  orphaned or multiply assigned components.
- Selected external tools parse their outputs and return normalized structured
  evidence.
- Variant and substitution fixtures prove deterministic inclusion and
  exclusion behavior.
- All upstream KiCad, simulation, routing where applicable, Bazel, and strict
  lockfile gates pass.

## Completion evidence

Not yet complete.
