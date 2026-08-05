# ADR-0010: Product resolution, BOM, placement, and assembly form one deterministic bundle

- Status: Accepted
- Date: 2026-08-04

## Context

[ADR-0008](0008-product-intent-and-pinned-catalog-evidence.md) establishes
source-authored product identities, approved substitutions, variants, and
configuration. [ADR-0009](0009-strict-offline-product-catalog-snapshot.md)
establishes strict offline catalog authentication and exact resolution. The
next layer must project one exact variant into useful product artifacts without
allowing a generated file, filesystem path, KiCad property, or mutually
consistent set of fabricated rows to become product authority.

BOM grouping, physical placement, and assembly population are different views
of the same selected component inventory. If each is generated or checked in
isolation, a fitted component can disappear from one view, a not-fitted
component can acquire quantity, or coordinated extra rows can reconcile with
each other while remaining absent from Design IR. The bundle therefore needs
one deterministic resolution spine and an independent verifier that rederives
every expected row from authoritative inputs.

## Decision

Layer 3 adds a public product-artifact compiler boundary. It consumes:

- one valid canonical Design IR v1 value;
- the exact product-catalog snapshot bytes named by that Design; and
- one exact variant semantic path.

The compiler does not trust a caller-supplied catalog-resolution object. It
re-runs the strict Layer-2 snapshot authentication and resolution checks before
projecting artifacts. It then requires the variant path to resolve to exactly
one authored variant and emits exactly four artifacts:

- `circuitc.product_resolution` v1;
- `circuitc.bom` v1;
- `circuitc.placement` v1; and
- `circuitc.assembly` v1.

The active unreleased Design IR remains schema version 1 and is unchanged by
this decision. Product artifacts are compiled projections, not fields added to
canonical Design IR.

## Artifact paths and common encoding

The compiler derives `variant_identity_sha256` as lowercase SHA-256 over these
exact bytes in order:

1. ASCII `CIRCUITC-PRODUCT-VARIANT-IDENTITY-V1`;
2. one NUL byte; and
3. the exact UTF-8 bytes of the variant semantic path.

The bundle contains exactly these normalized portable relative paths:

```text
product/<variant_identity_sha256>/resolution.json
product/<variant_identity_sha256>/bom.json
product/<variant_identity_sha256>/placement.json
product/<variant_identity_sha256>/assembly.json
```

The raw variant path never enters a filesystem path. Each artifact root carries
both the exact variant path and its recomputed identity digest. A path is
non-empty, `/`-separated, relative, and contains no empty, `.`, or `..`
component, backslash, NUL, or unrecognized bundle filename. The verifier
requires the exact four-path set and rejects extra, missing, aliased, or
misnamed files.

Every root also carries one common `product_input_sha256`. It is lowercase
SHA-256 over ASCII `CIRCUITC-PRODUCT-INPUT-V1`, one NUL byte, and a compact
canonical JSON preimage without final LF. In fixed field order, that preimage
contains Design name; catalog reference ID, SHA-256, and evaluation date;
variant path, build quantity, and sorted configurations; and every physical
component sorted by path. Each component contributes path, reference, logical
function, base manufacturer/part-number/package, exact typed value, lifecycle
requirement, sourcing minimum quantity/maximum lead time/region, the full
sorted approved-substitution list, exact placement X/Y/orthogonal rotation/side,
and the selected population state plus nullable exact alternate tuple. The
preimage is constructed from typed values only and never from Rust Debug,
source text or spans, output paths, filesystem state, host state, or collection
iteration order.

Every artifact is strict compact canonical UTF-8 JSON followed by exactly one
LF. Field order is schema order; unknown, missing, duplicate, reordered,
pretty-printed, trailing, or otherwise non-canonical fields or bytes are
invalid. Each complete artifact is at most 67,108,864 bytes including its final
LF, and each schema's primary row collection contains at most 10,000 rows.
Limits fail closed before expanded row or reconciliation validation. No
artifact is truncated or partially returned.

The checked sum of all four complete artifact byte lengths is also limited to
67,108,864 bytes. The compiler checks every row count, quantity, individual
serialized length, and aggregate serialized length before exposing any
artifact path or bytes; failure returns no partial bundle.

All four roots bind the Design name, exact variant path, variant-identity
digest, and common product-input digest. BOM, placement, and assembly
additionally bind the SHA-256 of the exact canonical resolution bytes,
including its final LF. Digests are 64 lowercase hexadecimal characters.

## Resolution spine

Product resolution contains the authenticated catalog snapshot ID, exact
snapshot-byte SHA-256, authored evaluation date, variant build quantity,
sorted configuration map, and exactly one row for every physical component.
Rows are sorted and unique by component semantic path and carry:

- exact component path and reference;
- the variant state `fitted`, `not_fitted`, or `alternate`;
- the component's exact base physical identity and typed value; and
- the exact selected identity and typed value, or `null` only when not fitted.

For `fitted`, selected identity equals base identity. For `alternate`, selected
identity is the exact approved alternate with the component's inherited logical
function and value and has already passed Layer-2 compatibility, lifecycle,
and sourcing checks. For `not_fitted`, selected identity is `null`. Virtual
components never appear.

## BOM, placement, and assembly views

The BOM groups fitted and alternate resolution rows by their exact selected
identity, including logical function, manufacturer, manufacturer part number,
package, and canonical typed value. Each group carries a positive checked
`u64` per-board quantity and `total_quantity = per_board_quantity *
build_quantity`, also checked in `u64`. Not-fitted and virtual components do
not contribute. BOM rows are strictly sorted and unique by selected identity.

Placement contains exactly one row for every fitted or alternate physical
component and no other component. Each row carries component path, reference,
selected identity, signed integer-nanometre `x_nm` and `y_nm`, orthogonal
rotation, and exact `front` or `back` side from Design IR. Rows are strictly
sorted and unique by component path. This is a canonical Design placement
projection, not a KiCad-derived or machine-specific pick-and-place file.

Assembly contains exactly one row for every physical component. It repeats the
exact state and nullable selected identity, carries `per_board_quantity` equal
to `0` for `not_fitted` and `1` otherwise, and carries the checked `u64` product
of that value and the root build quantity. Its root repeats the exact sorted
variant configurations and build quantity. Rows are strictly sorted and unique
by component path. This is population intent, not proof that an assembly host
accepted or executed it.

## Bidirectional reconciliation and verification

Identity joins are exact and bidirectional:

- every physical Design component has exactly one resolution and assembly row,
  and every such row names a physical Design component;
- every fitted or alternate resolution row has exactly one placement row, and
  every placement row names one such resolution row;
- every fitted or alternate resolution row contributes exactly one unit to one
  BOM group, every BOM group has at least one contributing resolution row, and
  grouped quantities equal the complete contributing inventory;
- not-fitted rows have null selected identity, zero assembly quantities, no
  placement row, and no BOM contribution;
- base and selected identities, component references, placement, state,
  configuration, build quantity, and all derived quantities equal their
  authoritative Design, variant, and authenticated-catalog inputs exactly; and
- no virtual component appears in any product artifact.

The independent strict bundle verifier consumes the same valid Design, exact
catalog snapshot bytes, exact variant path, and exact path-to-bytes bundle. It
re-authenticates and resolves the catalog, recomputes the variant identity and
all four expected artifacts, validates each schema and canonical encoding, and
requires byte equality with every expected artifact. It does not accept
cross-artifact consistency as sufficient evidence. Consequently, a coordinated
extra, omission, substitution, stale root, stale digest, reordered row, or
mutually rewritten bundle still fails when it is absent from the recomputed
Design-derived inventory.

The verifier uses an independent row-selection, population, grouping, quantity,
placement, and reconciliation derivation. It may share primitive schema
parsers, canonical serializers, checked arithmetic, and digest functions with
the emitter, but it may not call or reuse the emitter's selection, grouping,
projection, or bundle-construction helpers.

Any invalid input, overflow, path error, schema error, stale byte, catalog
failure, or reconciliation mismatch produces stable machine-readable
diagnostics and no partial bundle or partial verification result.

## Authority and layer boundary

CircuitC source and canonical Design IR own component, part, variant,
configuration, and placement intent. The exact pinned snapshot owns only its
authenticated observations. Layer-3 artifacts are deterministic compiler
outputs derived from those authorities; editing them cannot change intent.

Layer 3 does not invoke or trust KiCad, a placement host, an assembly host, a
distributor, the network, the host clock, or a filesystem enumeration order.
It does not emit fabrication data, claim host parse or manufacturing
acceptance, normalize manufacturability results, close a release manifest, or
publish a release. Those remain later layers with separate accepted contracts.

## Consequences

- One variant has one exact, safe, deterministic four-artifact bundle.
- Product rows cannot be added, removed, or coordinated across files without
  failing independent Design-derived verification.
- Exact integer quantities and coordinates never pass through floating point.
- BOM and assembly totals fail on overflow rather than wrapping or saturating.
- Product artifacts can become inputs to later manufacturing adapters without
  becoming source authority themselves.
