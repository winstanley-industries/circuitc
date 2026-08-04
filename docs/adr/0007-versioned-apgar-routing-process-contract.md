# ADR-0007: APGAR routing uses a versioned authenticated process contract

- Status: Accepted
- Date: 2026-08-03

## Context

EPIC-0004 adds route search without changing CircuitC's source-of-truth model.
An authored request to find a route is not itself copper, and a tool result
cannot become physical intent merely because a process exited successfully.
CircuitC must keep the request, selected geometry, and acceptance evidence
joined without embedding APGAR's C++ objects, compiled fields, or CUDA state in
the canonical Design IR.

The two projects also use different exact coordinate units. CircuitC stores
signed integer nanometres, while APGAR's current Board IR uses 2,000,000
database units per millimetre. The boundary must not round, truncate, or
silently approximate geometry or unsupported rules.

## Decision

The active unreleased Design IR v1 gains zero or one authored planar autoroute
request. Its source form is:

```text
autoroute <path> net <net> width <length> clearance <length> grid <length> layer <front|back>;
```

The request has a stable semantic path, names one declared net and one copper
layer, and carries positive exact width, clearance, and grid values in integer
nanometres. The grid is anchored at the board-outline origin, and both physical
terminal-pad centres must lie on it. It is distinct from an authored `route`
segment: `route` is canonical copper, while `autoroute` is unresolved routing
intent and cannot be emitted to KiCad as if it were already accepted.

The initial capability is deliberately closed. One request may route one net
with exactly two physical terminal pads on one selected front or back layer.
The deterministic CPU reference may return only continuous horizontal,
vertical, or 45-degree centreline segments. Vias, arcs, multipin routing,
multiple simultaneous routing profiles, other headings, and approximations of
unsupported geometry or rules are rejected with machine-readable diagnostics.

The request carries only that selected layer. Because the pinned APGAR M1
Board IR validates a complete two-signal-layer stack, the process adapter
deterministically materializes the unselected front-or-back companion layer.
Its identity derives from the selected layer under a versioned domain, it is
excluded from the allowed routing layers, and it participates in the APGAR
board-content fingerprint. This adapter-owned stack context cannot authorize
copper on the companion layer or widen the one-layer CircuitC capability.

CircuitC lowers nanometres to APGAR database units with checked multiplication
by two. Import performs the inverse check: every returned coordinate and width
must be exactly divisible by two and must fit the Design IR envelope. No
floating-point conversion is permitted at this boundary.

The initial APGAR integration is a process boundary with strict, canonical,
versioned JSON request and result contracts. Those wire schemas are separate
from Design IR v1 and will define exact key sets, canonical UTF-8 encoding,
checksums over exact bytes, request/result association, stable identities,
toolchain provenance, deterministic failure states, and bounded parsing. They
must not serialize APGAR implementation layouts. Identical canonical input to
the pinned CPU implementation must produce byte-identical contract artifacts.

Bazel pins the adapter to one immutable APGAR commit and checks that the build,
Rust verifier, and C++ adapter declare the same source identity. The CPU-only
consumer patch changes only APGAR module evaluation: it makes the unconditionally
loaded Python rules a regular dependency and omits the unused CUDA GCC module
extension that cannot be evaluated on the supported Darwin host. It does not
change APGAR routing source, public APIs, candidate evidence, or exact
admission. CUDA execution is outside this initial boundary and is not implied
to be validated by the CPU adapter.

A result remains untrusted routing evidence until CircuitC strictly parses it,
authenticates it against the exact request and toolchain, verifies the selected
candidate and APGAR exact-admission status, and losslessly imports only the
declared geometry subset. Successful import constructs and validates a fresh
canonical Design IR whose selected route segments are physical intent for the
remaining compiler phases. Raw APGAR output and an unauthenticated or stale
selection never become canonical intent.

APGAR exact admission and supported-host KiCad DRC are separate acceptance
authorities. APGAR establishes exact validity within its declared routing
subset. KiCad DRC establishes whether the emitted board is accepted by the
supported KiCad host. Every accepted imported route requires both results,
bound to the same authenticated request, selected result, imported Design IR,
and emitted board; neither result substitutes for the other.

CircuitC source remains the human-authored authority. The authored request
authorizes route search, the authenticated import supplies the selected exact
geometry to the canonical Design IR for that build, and generated KiCad files
remain deterministic outputs rather than an alternate editable source.

Static compiler APIs reject unresolved autoroute intent. Checked compilation
executes the complete route boundary before static and simulation lowering. On
success, the canonical request, authenticated result, and projection manifest
are published atomically with the exact generated board and all other accepted
artifacts. A routing failure publishes no accepted or checked-failure tree. If
routing succeeds but a later simulation fails, CircuitC discards routing and
static artifacts and may publish only the complete simulation evidence chains:
the projection is not meaningful without the exact board it binds.

## Consequences

- Design IR v1 evolves in place because it is still unreleased; no schema bump,
  migration, or compatibility adapter is introduced.
- Existing source-authored `route` segments continue to represent copper and
  do not require APGAR.
- An autoroute request can be parsed and validated independently of whether a
  later compiler phase is configured to execute APGAR.
- The deterministic CPU reference establishes the contract before GPU results
  can be accepted. A later accelerator must preserve the same exact request,
  result, authentication, import, and dual-validation semantics.
- Expanding beyond two-terminal, one-layer H/V/45 routing requires an explicit
  capability and contract change rather than a permissive fallback.
