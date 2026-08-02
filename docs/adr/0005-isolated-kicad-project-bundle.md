# ADR-0005: KiCad projects use a vendored catalog and identity manifest

- Status: Accepted
- Date: 2026-08-01

## Context

M1 emitted a PCB whose footprint geometry was authored inline. KiCad could
parse it, but host DRC warned because the `CircuitC` library was absent from
the user's global configuration. That cannot close the useful-project
milestone: a complete build must include symbols, footprints, project
configuration, schematic-to-PCB ownership, and source-correlated host evidence
without relying on editor state.

CircuitC also needs hierarchy, electrical pin types, and explicit no-connects
without making KiCad's symbol or sheet objects canonical. KiCad libraries are
backend resources and KiCad UUIDs are backend identities; both must remain
derived from canonical intent.

## Decision

- The active unreleased Design IR gains an elaborated module-instance tree,
  typed module ports, explicit connected/no-connect pin states, part identity,
  an exact typed component value independent of simulation, symbol-pin
  bindings, optional terminal-only simulator-model identifiers, and
  source-authored schematic placement.
- The first library catalog is vendored in this repository and compiled into
  the backend. A part resolves by its full logical-device, manufacturer, and
  manufacturer-part-number tuple; the catalog owns its compatible symbol and
  optional footprint. Elaboration copies exact footprint geometry into Design
  IR.
- The initial KiCad catalog supports the common symbol-pin-number equals
  footprint-pad-number convention. The Design IR retains explicit independent
  bindings, but KiCad lowering fails closed when a part requires a different
  pin-equivalence map; extending that representation requires a catalog
  contract change.
- One compile emits an isolated KiCad bundle: schematic, board, project JSON,
  local library tables, and the exact vendored symbol and footprint files.
  The library-file collection is ordered and derived from the catalog entries
  used by the design rather than represented by fixed singleton fields.
  Library tables use `${KIPRJMOD}` paths and never refer to user-global tables.
- Vendored footprint silkscreen and courtyard drawings remain KiCad catalog
  data outside canonical Design IR. Board lowering copies those drawings with
  deterministic per-component UUIDs so courtyard-overlap DRC is active for the
  bootstrap catalog.
- Schematic symbols and PCB footprints share a deterministically derived
  semantic identity. The PCB path points at the schematic symbol UUID so
  KiCad's schematic-parity check owns acceptance.
- Component schematic anchors are unique, and lowering rejects transformed
  pin connection-point collisions when their canonical connection states
  differ. Coincident labels cannot silently merge distinct CircuitC nets.
- Source compilation emits a deterministic KiCad identity manifest mapping
  UUIDs to globally unique semantic paths and UTF-8 source locations. Its
  source field is derived from the design name rather than the requested input
  path. Host-report normalization joins all findings through that manifest.
  Normalization requires canonical UUIDv8 and semantic-path forms, exact
  manifest fields, a matching logical source stem, and a manifest entry for
  every UUID-bearing finding; correlation is never best-effort.
- Canonical no-connect pads have no CircuitC net. KiCad PCB lowering assigns a
  deterministic, backend-only `unconnected-(<ref>-Pad<pad>)` net to preserve
  the authored open across KiCad schematic-to-PCB parity.
- The M1B host gate runs both ERC and DRC, requests schematic parity, rejects
  unconnected or unexpected findings, and uses an isolated KiCad configuration
  directory. It parses schematic, PCB, symbol, and footprint artifacts with
  KiCad 10. Because `kicad-cli` exposes no direct project-file parser, CircuitC
  validates the emitted `.kicad_pro` JSON against its exact generated subset.
  A successful host exit status alone is still insufficient.

## Consequences

- Adding a supported library part requires a reviewed vendored asset plus a
  matching catalog entry, drawing geometry, publishable-file mapping, and
  ingestion tests.
- Display names and library file order do not participate in canonical entity
  identity.
- KiCad-specific symbol geometry remains outside Design IR; only the explicit
  logical-to-library pin contract enters canonical intent.
- A source file can express physical-only parts by omitting both model and
  terminals. It can express virtual parts by explicitly omitting manufacturer
  identity. Every physical part requires both a manufacturer and manufacturer
  part number.
- Parameterized reusable source modules may be added later by elaborating to
  the same module-instance tree; no KiCad sheet object becomes canonical.
