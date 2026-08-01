# CircuitC Agent Guide

This file governs work throughout the repository.

## Read before changing code

1. Read `docs/architecture.md`.
2. Read the active or planned epic under `docs/epics/` that owns the outcome.
3. Read the accepted decisions under `docs/adr/` that touch the change.
4. Read the versioned schema contract under `schemas/` before changing an IR.

Record intentional changes to compiler boundaries, authority, determinism, or
backend ownership in an ADR or the architecture document. Do not let an
implementation silently redefine those contracts.

## Non-negotiable contracts

- CircuitC source and canonical Design IR are authoritative; generated KiCad
  and SPICE files are deterministic artifacts.
- Coordinates are exact signed integer nanometres in the Design IR. Floating
  point values may only appear after an explicit backend conversion.
- Electrical quantities are exact decimal values with dimensions until a
  simulator backend explicitly lowers them.
- Stable identities derive from semantic paths or explicit source identities,
  never wall-clock time, randomness, filesystem location, or iteration order.
- Unsupported input must fail with a machine-readable diagnostic. Backends may
  not silently weaken electrical, geometric, or simulation semantics.
- KiCad's parser, ERC, and DRC are the final authority for KiCad output.
- APGAR and Ohmnivore integrate through versioned contracts; their internal IRs
  do not become CircuitC's canonical IR by accident.
- CircuitC remains headless. Do not add a GUI or editor-specific core
  dependency without an explicit architecture decision.
- Bazel is the canonical and exclusive top-level build interface.

## Engineering discipline

- Prefer validated vertical slices with observable inputs and outputs over
  empty subsystem scaffolding.
- Keep generated files byte-deterministic and add repeat-build tests.
- Keep serialized schemas versioned and document compatibility policy.
- Until CircuitC publishes its first released schema, evolve the active Design
  IR schema in place without version bumps or backwards-compatibility work.
  Record semantic changes, but do not create migrations for unreleased forms.
- Preserve unrelated worktree changes and stage files explicitly.
- Put temporary probes and cross-agent artifacts under `.agent-scratch/`.

## Required validation

Run the narrowest relevant tests while developing, then:

```sh
bazel build //...
bazel test //...
bazel test --lockfile_mode=error //...
```

KiCad backend changes additionally require parsing the generated artifact with
the supported `kicad-cli` version and producing a structured ERC or DRC report.
Report any unavailable gate exactly; do not present an unexecuted gate as
passing.
