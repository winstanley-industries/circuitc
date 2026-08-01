# ADR-0003: The unreleased Design IR evolves in place

- Status: Accepted
- Date: 2026-08-01

## Context

CircuitC has not released a Design IR schema or serialized wire format. The v1
bootstrap derived route UUIDs from geometry, implicitly equated logical pin
names with footprint pad numbers, and used ambiguous dot-concatenated UUID
inputs. Correcting those decisions changes bootstrap fields and identity.

Bumping schema versions or implementing compatibility for every pre-release
correction would preserve forms that no external release has promised. The
public Rust structs are likewise a convenient executable bootstrap, not a
stable source API.

Unchecked rotation negation and incomplete envelope checks also let publicly
constructible IR values panic validation or escape the documented coordinate
domain.

## Decision

Until CircuitC publishes its first released schema, the active Design IR stays
at schema version 1 and evolves in place. Pre-release changes do not require
backwards compatibility, migrations, or version bumps. Semantic changes remain
documented in the active schema and in ADRs when they affect compiler
boundaries, authority, determinism, or backend ownership.

As part of this correction:

- routes carry semantic paths independent of geometry;
- physical implementations carry explicit logical-pin-to-pad bindings;
- KiCad identity hashing consumes a domain-tagged, length-prefixed field
  sequence and emitted UUIDs are checked globally for collisions; and
- all stored and derived coordinate-bearing values are envelope-validated with
  checked arithmetic, returning diagnostics rather than panicking.

## Consequences

- Bootstrap fixtures and Rust callers move with the active schema immediately.
- No compatibility adapters or obsolete schema copies are maintained before a
  release creates a real consumer contract.
- Logical symbol pins no longer need names equal to physical pad numbers.
- Moving a route preserves its backend identity; duplicating undirected copper
  geometry remains an error.
- The first schema release must explicitly define its versioning and
  compatibility policy before external consumers rely on it.
