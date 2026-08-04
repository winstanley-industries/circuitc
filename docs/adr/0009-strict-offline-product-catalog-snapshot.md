# ADR-0009: Product catalog evidence is strict, pinned, and offline

- Status: Accepted
- Date: 2026-08-04

## Context

[ADR-0008](0008-product-intent-and-pinned-catalog-evidence.md) separates
source-authored product intent from point-in-time catalog observations. It
also requires a physical Design to name the exact evidence snapshot and the
authored date on which that evidence is evaluated. The evidence format and the
rules that join it back to Design IR must now be fixed before BOM or
manufacturing work can depend on them.

A permissive JSON document, live distributor lookup, or best-effort part match
would weaken that authority split. Reordered or normalized evidence bytes
could evade the Design IR digest, a package-only match could approve the wrong
electrical value, and host-clock freshness would make an identical build
change meaning over time.

## Decision

CircuitC accepts product-catalog observations only through the versioned
`circuitc.product_catalog_snapshot` v1 contract documented at
[`schemas/product_catalog_snapshot/v1.md`](../../schemas/product_catalog_snapshot/v1.md).
The snapshot is compact canonical UTF-8 JSON followed by exactly one LF. Its
complete canonical artifact, including that LF, is limited to 64 MiB. A
consumer rejects unknown, missing, duplicate, reordered, pretty-printed, or
otherwise non-canonical fields or bytes by parsing, validating, canonically
reserializing, and requiring exact byte equality.

The snapshot header contains:

- the exact schema name and version;
- a canonical `snapshot_id`;
- real Gregorian `observed_on` and `valid_through` dates, with
  `valid_through >= observed_on`;
- a narrow canonical ASCII HTTPS `source_uri` with lowercase DNS authority,
  no normalization-bearing dot segment, and no IPv6-literal form; and
- the lowercase SHA-256 digest of the raw upstream source from which the
  snapshot was prepared.

The source URI and raw-source digest are traceability only. They do not prove
external truth or upstream authenticity unless the exact raw bytes are
separately retained and verified. Resolution is offline and never dereferences
the URI or fetches raw evidence during a build.

A snapshot contains at most 10,000 strictly sorted, unique part records and at
most 10,000 regional observations in aggregate. A part record has the exact
logical function, manufacturer, manufacturer part number, package, typed exact
value, observed lifecycle, and sorted unique regional availability. A value is
either `resistance` or `dc_voltage` and carries a canonical signed decimal
coefficient, exponent, and unit without floating-point conversion. A regional
observation names the exact region, available quantity, and lead time in days.

Part records sort and are unique by the exact
`(logical_function, manufacturer, manufacturer_part_number, package)` key.
Regional observations sort and are unique by exact region. Declaration order
is therefore authenticated rather than normalized after verification.

## Verification and resolution

Catalog resolution takes one already-valid Design IR and the exact snapshot
bytes named by its `CatalogEvidenceRef`. It performs these checks without
network access or host-clock input:

1. Reject an artifact over the complete 64 MiB byte ceiling before hashing or
   parsing it.
2. Hash the exact snapshot bytes, including the final LF, and require equality
   with `CatalogEvidenceRef.sha256`.
3. Parse and validate the complete snapshot contract and canonical bytes.
4. Require the header `snapshot_id` to equal the Design IR `snapshot_id`.
5. Require the authored Design IR `evaluated_on` date to be inside the closed
   `[observed_on, valid_through]` interval.
6. Resolve every primary physical part and every source-approved substitute to
   exactly one record by exact logical function, manufacturer, manufacturer
   part number, and package.
7. Require exact logical-function, package, and canonical typed-value
   compatibility. Approved-substitution status alone is not compatibility
   evidence.
8. Require observed lifecycle to equal the component's authored lifecycle
   requirement.
9. Require the authored region to exist and its observed quantity and lead
   time to satisfy the authored minimum-quantity and maximum-lead-time bounds.

Each approved substitute inherits the component's authored logical function,
value, lifecycle requirement, and sourcing constraints for these checks.
There is no fuzzy manufacturer, part-number, package, function, value, region,
or lifecycle matching and no selection of an unapproved substitute.

Validation and resolution are all-or-nothing. Any contract, authentication,
freshness, inventory, compatibility, lifecycle, or sourcing failure produces a
stable machine-readable `CC-CATALOG-*` diagnostic and no resolved catalog.
Oversized collections fail before expanded record and region validation. A
consumer may not truncate, partially resolve, or return the entries that
happened to pass.

Resolution preflights the aggregate number of primary and approved-alternate
identities across physical components and rejects more than 10,000 before
building its result or visiting individual identities. It builds one exact
identity index over the validated snapshot, visits components and alternates
in canonical identity order, and therefore does not multiply catalog size by
resolution-request count or inherit caller collection order in diagnostics.

## Authority and layer boundary

CircuitC source and Design IR remain authoritative for primary identities,
approved substitutions, exact values, lifecycle requirements, sourcing
constraints, variants, and evaluation date. The authenticated snapshot owns
only its declared point-in-time observations and validity interval. Resolution
proves that those observations satisfy the authored intent; it cannot add or
rewrite intent.

This layer exposes snapshot parsing, authentication, and deterministic
resolution only. A resolved catalog is compiler evidence for later layers, not
a BOM, procurement instruction, manufacturing artifact, analysis report,
release manifest, or published release. Those outputs require their own
accepted contracts and validation joins.

## Consequences

- Identical Design IR and snapshot bytes resolve identically offline.
- Snapshot freshness is an authored, digest-bound interval decision rather
  than a wall-clock decision.
- Exact-byte pinning detects any record, provenance, ordering, or encoding
  change.
- Catalog evidence cannot silently approve an electrically incompatible or
  cross-package alternate.
- Refreshing observations requires a new snapshot digest and an explicit
  Design IR reference change.
