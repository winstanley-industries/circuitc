# CircuitC Epics

Epics turn the architecture roadmap into durable, testable product outcomes.
They define why a capability exists, what must be true when it is complete, and
which evidence closes it. They intentionally do not contain short-lived task
lists or implementation assignments.

## Index

| Epic | Status | Architecture milestone | Outcome |
| --- | --- | --- | --- |
| [EPIC-0000](EPIC-0000-architecture-spine.md) | Complete | M0 and M0.1 | Validated Rust/Bazel compiler spine with deterministic KiCad and SPICE backends |
| [EPIC-0001](EPIC-0001-language-frontend.md) | Complete | M1A | File-authored CircuitC design compiled through the existing backends |
| [EPIC-0002](EPIC-0002-kicad-project-compiler.md) | Complete | M1B | Complete, reproducible KiCad schematic, PCB, and project generation |
| [EPIC-0003](EPIC-0003-simulation-closure.md) | Complete | M2 | Simulation models, analyses, and assertions as checked compiler phases |
| [EPIC-0004](EPIC-0004-apgar-routing-integration.md) | Complete | M3 | APGAR route generation imported with exact validation and provenance |
| [EPIC-0005](EPIC-0005-product-and-manufacturing.md) | Planned | M4 | Reproducible product, BOM, fabrication, assembly, and release outputs |

## Document authority

- `docs/architecture.md` owns system boundaries, authority, and the roadmap.
- `docs/epics/` owns durable outcomes, requirements, dependencies, and
  completion evidence.
- `docs/adr/` owns accepted architecture decisions and their consequences.
- `schemas/` owns active data contracts.
- GitHub issues and execution prompts own implementation tasks and sequencing.

An epic may clarify a roadmap outcome but may not silently redefine an
architecture or schema contract. A conflicting requirement requires the
appropriate ADR, schema, or architecture change.

## Lifecycle

Epics use one of four statuses:

- **Planned:** the outcome is accepted, but implementation is not active.
- **Active:** at least one validated vertical slice is being pursued.
- **Complete:** every requirement has named completion evidence.
- **Superseded:** another linked epic replaces the outcome.

Status is evidence-based. Starting code does not make an epic complete, and a
passing local test is not a substitute for every authority named by the epic.

## Requirement identifiers

Requirements use identifiers such as `CC-REQ-LANG-001`. An identifier is not
reused for a different meaning after an issue, commit, test, or release refers
to it. Before the first CircuitC release, requirement wording may be corrected
in place, consistent with the unreleased-schema policy, but semantic changes
must remain visible in Git history.

Each implementation issue should name the requirement identifiers it covers.
Each requirement should be covered by one or more issues without copying the
issue's task checklist back into the epic.

## Completion evidence

Completion evidence should identify durable proof, such as:

- a commit or release containing the implementation;
- exact Bazel targets and test results;
- a supported host-tool report;
- a deterministic golden or repeat-build comparison; or
- an on-target integration result for APGAR, Ohmnivore, or manufacturing tools.

Record unavailable gates explicitly. Do not mark a requirement complete on the
basis of an unexecuted authority check.
