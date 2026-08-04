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

## Layer 3: deterministic product artifact bundle

[ADR-0010](../adr/0010-deterministic-product-artifact-bundle.md) defines a
public compiler boundary that consumes valid Design IR, exact pinned catalog
snapshot bytes, and one exact variant path. It re-runs Layer-2 verification and
emits exactly four strict schemas:

- [`product_resolution` v1](../../schemas/product_resolution/v1.md) contains
  every physical component exactly once with state, base identity, and nullable
  selected identity;
- [`bom` v1](../../schemas/bom/v1.md) groups fitted selected identities and
  exact typed values with checked per-board and total `u64` quantities;
- [`placement` v1](../../schemas/placement/v1.md) contains every fitted physical
  component exactly once with signed integer nanometres, orthogonal rotation,
  and front/back side; and
- [`assembly` v1](../../schemas/assembly/v1.md) contains every physical
  component exactly once with state, nullable selection, checked quantities,
  build quantity, and exact configurations.

Each artifact is compact canonical JSON plus LF, individually limited to 64
MiB and 10,000 primary rows. Their checked aggregate complete size is also
limited to 64 MiB and is preflighted before any path or bytes are returned.
Paths are exactly
`product/<variant_identity_sha256>/{resolution,bom,placement,assembly}.json`;
the digest is domain-separated SHA-256 of the exact variant path.

All roots bind Design name, exact variant path and digest, and one common
domain-separated `product_input_sha256`. Its canonical preimage covers catalog
reference, variant path/build/configurations, and every physical component's
sorted identity, exact value, lifecycle and sourcing constraints, full
approved substitutions, placement, and selected population state. BOM,
placement, and assembly also bind the exact resolution digest.

All identities reconcile bidirectionally; not-fitted components have no BOM or
placement contribution and zero assembly quantities, and virtual components
never appear. The independent strict verifier recomputes expected joins and
bytes from Design plus freshly authenticated catalog evidence without sharing
the emitter's selection, grouping, quantity, placement, or bundle helpers. It
rejects missing, reordered, duplicate, stale, coordinated-extra, or partially
valid bundles.

Layer 3 leaves Design IR v1 unchanged and claims no KiCad or manufacturing-host
authority. Fabrication output, host analysis evidence, release-manifest
closure, and publication remain later layers.

## Layer 4: deterministic KiCad fabrication evidence

[ADR-0011](../adr/0011-deterministic-kicad-fabrication-evidence.md), the
[`circuitc.fabrication_request` v1 schema](../../schemas/fabrication_request/v1.md),
and the
[`circuitc.fabrication_manifest` v1 schema](../../schemas/fabrication_manifest/v1.md)
define the first fabrication-host boundary:

- one request binds exact Design/analysis/assertion/variant identity, verified
  Layer-3 product roots, exact generated PCB bytes, a fixed KiCad 10.0.5 export
  profile, and an exact 13-file native inventory under a digest-derived safe
  path;
- static compiler artifacts are independently reproduced, simulation-bearing
  checked boards are independently lowered, and routed checked boards are
  deterministically replayed from current Design plus opaque route evidence;
- KiCad exports nine Gerber X2 manufacturing layers plus its job file, separate
  PTH and NPTH Excellon files, and one both-side all-footprint position CSV;
- Design IR v1 has no hole construct, so v1 requires explicit zero-tool,
  zero-hit PTH and NPTH files and fails on nonzero drill output;
- the strict parser proves Gerber/job file-function parity, host/project
  identity, Excellon policy, and full physical-reference/coordinate/side/
  rotation equality with Design; product population remains Layer-3 authority,
  so not-fitted footprints may appear only as full-board parity rows;
- KiCad 10.0.5 embeds wall-clock creation fields despite
  `SOURCE_DATE_EPOCH`; CircuitC validates and rewrites only those exact fields
  to the authenticated catalog evaluation date at midnight, keeps raw host
  files transient, and binds every normalized byte; and
- a private no-follow host snapshot and exact explicit inventory prevent source
  replacement, symlink substitution, user configuration, or directory scans
  from defining accepted outputs; the validated request drives the host,
  executable identity is computed from exact bytes, raw publication is one
  atomic no-replace rename, and the transient request/board/executable/output
  receipt and held no-follow directory namespace are rechecked before the
  verified manifest is emitted.

Layer 4 does not evaluate ERC, DRC, unconnected, or schematic-parity assertions
and does not close or publish a release. Those remain later layers.

## Layer 5: capability-declared KiCad board analysis

[ADR-0012](../adr/0012-capability-declared-kicad-board-analysis.md) and the
[`board_analysis_request`](../../schemas/board_analysis_request/v1.md),
[`board_analysis_result`](../../schemas/board_analysis_result/v1.md), and
[`board_analysis_report`](../../schemas/board_analysis_report/v1.md) v1
contracts define the first structured product-analysis adapter:

- one domain-separated request binds the exact generated schematic, PCB,
  identity map, expected ERC sheet inventory, complete compiler-emitted KiCad
  project support inventory, current-input-authenticated Layer-4 fabrication
  predecessor, fixed KiCad 10.0.5 policy, resource limits, and exactly one
  authored assertion for each of the five initial capabilities;
- immutable host execution produces separately bound normalized ERC and DRC
  reports plus a receipt for the exact request, inputs, executable,
  normalizer, host runner, and report bytes;
- ERC violations, DRC findings, unconnected items, schematic-parity findings,
  and fabrication completeness remain separate evidence checks and separate
  assertion outcomes;
- completed results contain one indivisible ERC/DRC/fabrication evidence set
  and independently report `pass` or `fail` for every capability, while failed
  or unsupported results contain no partial tool or evidence object and make
  every assertion explicitly unevaluated or unsupported; and
- an independent verifier recomputes the complete request, result, report, and
  evidence bundle from current Design, product, compiler, fabrication, host,
  and normalizer inputs.

The host gate stages only request-authenticated project bytes and executes
authenticated tool snapshots under bounded, isolated processes. The live Bazel
gate executes KiCad ERC and DRC twice on separately compiled projects and
requires byte-identical normalized reports and five-outcome analysis reports.
Layer 5 does not bind the complete release inventory,
simulation or route acceptance applicability, source identity, or transactional
closure. Release closure remains Layer 6 and filesystem publication remains
Layer 7.

## Layer 6: verified content-addressed release closure

[ADR-0013](../adr/0013-content-addressed-release-closure.md), the
[`release_request`](../../schemas/release_request/v1.md), and
[`release_manifest`](../../schemas/release_manifest/v1.md) v1 contracts define
the distributable release boundary:

- exact CircuitC source is re-elaborated and joined to a separately bound,
  complete canonical Design identity;
- the binder independently reverifies current catalog, product, fabrication,
  and five-capability board-analysis evidence rather than trusting caller
  checksums or `all_pass` fields;
- checked simulation and APGAR routing artifacts and exact external tool
  provenance are required exactly when the Design declares those capabilities,
  while ordinary authored route segments do not imply APGAR applicability;
- the complete artifact and tool inventory is derived from typed predecessor
  bundles with strict safe paths, count, per-file, path, and checked aggregate
  limits; and
- the independently verified request, manifest, and exact payload bytes form
  one opaque in-memory content-addressed closure with no caller-authored
  inventory or host path.

Layer 6 closes `CC-REQ-PROD-006` and `CC-REQ-PROD-007`. It does not make a
network service, mutable release channel, package registry, or filesystem
transaction authoritative.

## Layer 7: immutable transactional materialization

Layer 7 consumes only an independently verified Layer-6 closure and writes its
exact tree into a private sibling staging directory. It writes the manifest
completion sentinel last, synchronizes every file and directory, and exposes
the complete content-addressed root with one atomic no-replace rename. Existing
destinations of any type are immutable. Namespace ownership, mode, ACL,
no-follow traversal, held identity, rollback, concurrency, and post-rename
durability-warning behavior follow the hardened CircuitC publication boundary;
packaging, signing, upload, and registries remain future separate authorities.

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
