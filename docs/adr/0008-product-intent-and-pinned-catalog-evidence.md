# ADR-0008: Product intent and pinned catalog evidence remain separate authorities

- Status: Accepted
- Date: 2026-08-04

## Context

EPIC-0005 adds product variants, sourcing constraints, approved substitutions,
and manufacturability checks. These concepts cannot be inferred from KiCad
display names or from a live distributor response. CircuitC also needs to keep
the human-authored product decision separate from point-in-time catalog and
lifecycle observations while still binding a reproducible build to the exact
evidence it evaluated.

The existing bootstrap part identity combines a logical-device token with an
optional manufacturer and manufacturer part number. That is sufficient to
select the initial KiCad catalog entry, but it does not distinguish logical
function from package, lifecycle policy, sourcing policy, substitution
approval, or per-product population state.

## Decision

The active unreleased Design IR remains at schema version 1 and evolves in
place under [ADR-0003](0003-unreleased-design-ir-evolves-in-place.md). The
field previously called `logical_device` is renamed `logical_function`. No
migration, compatibility adapter, or schema-version bump is introduced.

Every physical part carries all of the following canonical authored intent:

- `logical_function`;
- manufacturer and manufacturer part number;
- package;
- a lifecycle requirement of `active`,
  `not_recommended_for_new_designs`, or `obsolete`;
- sourcing constraints containing a positive `minimum_available_quantity`, a
  positive `maximum_lead_time_days`, and a canonical `required_region`; and
- a sorted, duplicate-free set of at most 256 exact approved substitutions,
  each naming a manufacturer, manufacturer part number, and package.

An approved substitution must use exactly the primary part's package. The
exact primary `(manufacturer, manufacturer_part_number, package)` tuple is not
a substitution and is invalid if repeated in the approved set. This package
equality is only a structural product-intent guard. A later authenticated
catalog-evidence layer must independently prove that an approved substitute is
compatible with the component's logical function and authored value; this
layer does not infer or claim that compatibility.

These fields are independent. A footprint or package display name cannot imply
a manufacturer identity, lifecycle decision, sourcing rule, or substitution.
Virtual parts carry `logical_function` but omit manufacturer, manufacturer
part number, package, lifecycle requirement, sourcing constraints, and
approved substitutions.

Every Design contains a `ProductIntent`. It has an optional catalog-evidence
reference, product variants, and manufacturability analyses. A design with at
least one physical component requires exactly one catalog-evidence reference
and at least one variant. A virtual-only design may omit the evidence reference
and variants.

A catalog-evidence reference contains a canonical `snapshot_id`, the lowercase
SHA-256 digest of the exact future snapshot-contract bytes, and an authored,
calendar-valid `evaluated_on` date in `YYYY-MM-DD` form. The reference is
canonical intent about which evidence was evaluated; remote lifecycle,
availability, and lead-time observations remain evidence outside the Design
IR. Later catalog contracts must authenticate their bytes against this
reference and evaluate freshness only from authored contract dates and policy.
They may not consult wall-clock time or the network during a build.

Each variant has a unique semantic path, a positive `u64` build quantity,
exactly one population state for every physical component, and a sorted,
duplicate-free configuration map. A state is one of:

- `fitted`, selecting the component's primary physical identity;
- `not_fitted`; or
- `alternate`, selecting an exact manufacturer, manufacturer-part-number, and
  package tuple that occurs in that component's approved substitutions.

Variants may not assign a population state to a virtual component. Component,
variant, substitution, and configuration declaration order has no semantic
meaning.

Product intent is bounded before expanded validation: a Design contains at
most 256 variants; each variant contains at most 256 configuration entries;
configuration keys contain at most 128 UTF-8 bytes and values at most 4096
UTF-8 bytes. The checked sum of the physical-component-count times
variant-count totality workload and all submitted component-state assignments
must be at most 10,000. This bounds validation even when a malformed input
omits required assignments or supplies extras.

The initial canonical manufacturability intent names a unique analysis path,
the exact adapter `kicad`, major version `10`, and stable assertion paths over
the closed assertion-kind set `erc_clean`, `drc_clean`, `unconnected_clean`,
`schematic_parity_clean`, and `fabrication_inventory_complete`. Assertions are
sorted and unique by semantic path, and one analysis may request each assertion
kind at most once. This decision defines authored intent and capability
selection only. KiCad manufacturing export, normalized analysis results,
assertion reports, release manifests, and release publication require later
ADRs and versioned contracts before they can be claimed as implemented.
One Design contains at most 256 manufacturability analyses and at most 10,000
assertions in aggregate across those analyses.

All collection, aggregate-workload, and UTF-8 byte ceilings are fail-closed
preflight checks. An oversized input is rejected before expanded membership,
variant-totality, per-entry semantic, or cross-entry validation; validation
does not partially accept or truncate it.

## Authority and validation rules

- CircuitC source and canonical Design IR own product policy, allowed
  substitutions, variant population, configuration, and requested
  manufacturability assertions.
- A pinned catalog snapshot owns only its authenticated point-in-time remote
  observations. It cannot add a substitution that source did not approve or
  relax source constraints. It must also prove logical-function and authored-
  value compatibility for an approved substitution before that substitution
  can become manufacturing evidence.
- Generated BOM, placement, fabrication, assembly, analysis, and release files
  remain deterministic compiled artifacts or normalized evidence. They do not
  become source authority.
- Unsupported lifecycle, sourcing, substitution, variant, or analysis intent
  fails with a stable machine-readable diagnostic. A backend may not silently
  choose a nearby part, package, region, adapter, version, or assertion.
- Stable identity and output order derive from semantic paths and exact
  canonical fields, never source order, filesystem paths, wall-clock time,
  randomness, remote lookup order, or hash-map iteration order.

## Consequences

- Product intent can be reviewed without trusting a live marketplace or
  generated KiCad properties.
- Identical source and pinned evidence can be rebuilt offline and evaluated
  deterministically on a later date.
- A catalog refresh changes explicit evidence identity and cannot silently
  alter a release.
- Supporting another manufacturability adapter or major version requires an
  explicit capability and contract change.
- This ADR does not define manufacturing file formats, host invocation,
  normalized result schemas, release-manifest closure, or transactional
  publication. Those remain planned EPIC-0005 layers.
