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

## Layer 1: authored product-intent boundary

[ADR-0008](../adr/0008-product-intent-and-pinned-catalog-evidence.md) defines
the first product-intent boundary while this epic remains Planned. The active
unreleased Design IR evolves in place at version 1; no version bump, migration,
or compatibility adapter is introduced.

- `PartIdentity.logical_device` becomes `logical_function`.
- Every physical part separately authors manufacturer, manufacturer part
  number, package, a lifecycle requirement, sourcing constraints, and exact
  approved substitutions. Lifecycle requirement is one of `active`,
  `not_recommended_for_new_designs`, or `obsolete`. Sourcing requires positive
  `u64` minimum available quantity, positive `u32` maximum lead-time days, and
  a canonical required-region token. Approved substitutions are sorted,
  unique exact manufacturer, manufacturer-part-number, and package tuples,
  with at most 256 entries per part. Every approved substitution package must
  exactly equal the primary package, and the exact primary tuple is invalid as
  a self-substitution. A later authenticated catalog layer, not this initial
  product-intent layer, proves logical-function and authored-value
  compatibility.
- Virtual parts retain logical function and omit manufacturer, manufacturer
  part number, package, lifecycle, sourcing, and substitution fields.
- Every Design has `ProductIntent`: an optional catalog-evidence reference,
  variants, and manufacturability analyses. A design containing a physical
  component requires the evidence reference and at least one variant.
- The evidence reference contains a canonical snapshot ID, the SHA-256 digest
  of the exact future snapshot-contract bytes, and an authored, calendar-valid
  `YYYY-MM-DD` evaluation date. Remote observations stay outside Design IR.
  Freshness is evaluated from authenticated authored data and policy, never
  wall-clock time or a network lookup during the build.
- Every variant has a unique path, a positive `u64` build quantity, exactly one
  fitted, not-fitted, or exact approved-alternate state for every physical
  component, and unique canonically ordered configuration key/value pairs.
  A Design has at most 256 variants, each with at most 256 configurations.
  Configuration keys contain at most 128 UTF-8 bytes and values at most 4096
  UTF-8 bytes. The checked sum of physical-component-count times variant-count
  totality work and all submitted component assignments is bounded at 10,000.
- The initial manufacturability intent permits only adapter `kicad`, major
  version `10`, with stable assertions over `erc_clean`, `drc_clean`,
  `unconnected_clean`, `schematic_parity_clean`, and
  `fabrication_inventory_complete`. A Design has at most 256 analyses and at
  most 10,000 assertions in aggregate.

Collection counts, checked aggregate workloads, and UTF-8 byte ceilings are
fail-closed preflight checks. Oversized intent is rejected before expanded
membership, totality, per-entry semantic, or cross-entry validation and is
never truncated or partially accepted.

This capability boundary establishes authored intent only. Parsing,
elaboration, and Design IR validation of these forms are not manufacturing
execution or completion evidence for this epic.

## Layer 2: strict offline catalog evidence

[ADR-0009](../adr/0009-strict-offline-product-catalog-snapshot.md) and the
[`circuitc.product_catalog_snapshot` v1 schema](../../schemas/product_catalog_snapshot/v1.md)
define the catalog-evidence boundary:

- The snapshot is strict canonical compact JSON plus one final LF, limited to
  64 MiB, 10,000 sorted unique part records, and 10,000 aggregate sorted unique
  regional observations. Unknown, missing, duplicate, reordered, or otherwise
  non-canonical fields or bytes are invalid.
- Its header binds snapshot identity, a real inclusive observation/validity
  date interval, a narrow canonical ASCII HTTPS source URI, and a lowercase
  raw-source SHA-256. URI and raw digest provide traceability only; resolution
  neither fetches the URI nor claims upstream authenticity without separately
  retained raw bytes.
- Each record binds an exact logical function, manufacturer, manufacturer part
  number, package, canonical typed resistance or DC-voltage value, observed
  lifecycle, and exact regional quantity/lead-time observations.
- The resolver authenticates the complete snapshot bytes and ID against Design
  IR, checks the authored evaluation date inside the inclusive interval, and
  resolves every primary and approved alternate exactly. Function, package,
  value, lifecycle, required region, minimum quantity, and maximum lead time
  must all satisfy authored intent.
- Resolution preflights at most 10,000 aggregate primary and alternate
  identities, indexes the validated snapshot once, and visits semantic
  identities in canonical order rather than caller order.
- Verification and resolution are offline, deterministic, fail with stable
  diagnostics, and return no partial result.

Layer 2 exposes verification and resolution only. BOM, placement, assembly,
fabrication export, normalized KiCad manufacturing results and reports,
release-manifest closure, and transactional release publication remain later
implementation layers requiring their own accepted contracts. Layer-2 parsing
or resolution is not manufacturing execution or completion evidence.

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
