# ADR-0004: Backends own names and normalized host evidence

- Status: Accepted
- Date: 2026-08-01

## Context

SPICE and current Ohmnivore do not accept the full canonical CircuitC token
space. They also reserve node `0`, treat `GND` as ground, and have
case-insensitive identity domains. Emitting canonical names verbatim can merge
distinct nets or cause the target parser to reject an otherwise valid design.

KiCad is the authority for generated board acceptance, but its raw structured
reports contain a run date and its process can exit successfully while still
reporting violations. Manual commands outside Bazel neither enforce policy nor
satisfy CircuitC's exclusive top-level build interface.

## Decision

- SPICE lowering preserves a canonical name only when it is supported,
  non-reserved, and unique under simulator case-folding. Other names receive a
  deterministic ASCII-safe name with collision resolution.
- Generated netlists include machine-readable mapping comments, and compiled
  artifacts expose the same net and device mapping structurally.
- Ground status comes only from the Design IR flag. Non-ground `0` and `GND`
  names never lower to simulator ground.
- KiCad host validation is invoked through a Bazel target that verifies KiCad
  10, parses JSON, rejects unconnected items and unexpected findings, and does
  not trust exit status alone.
- Raw KiCad reports are ephemeral evidence. CircuitC removes timestamps and
  host paths, canonicalizes ordering, and writes a deterministic normalized
  summary.

## Consequences

- Simulator results must be joined through the explicit name map instead of
  assuming emitted tokens equal source names.
- Backend-specific naming limitations do not leak into the canonical IR.
- Host validation remains intentionally local to an installed KiCad tool, but
  Bazel owns invocation and policy.
