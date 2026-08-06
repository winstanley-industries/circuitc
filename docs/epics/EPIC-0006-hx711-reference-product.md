# EPIC-0006: SparkFun HX711 reference-product qualification

- Status: Planned
- Architecture milestone: M5
- Depends on: EPIC-0005

## Outcome

CircuitC authors a functionally and mechanically equivalent, independently
manufacturable version of the SparkFun HX711 Load Cell Amplifier v11 and takes
it through deterministic KiCad 10 compilation, product resolution,
fabrication, board analysis, and immutable release closure.

The outcome qualifies CircuitC against a small real open-hardware product
rather than another compiler-owned synthetic fixture. The CircuitC source and
Design IR remain authoritative for the reproduced board. The pinned upstream
Eagle design is an attributed fidelity oracle, never a build-time importer,
compiler input, editable alternate authority, or substitute for KiCad host
acceptance.

## Reference selection

The selected reference is SparkFun's
[HX711 Load Cell Amplifier](https://github.com/sparkfun/HX711-Load-Cell-Amplifier)
revision v11 at upstream commit
[`38228f8f6602e1349d7130c7a382a436ceaca26e`](https://github.com/sparkfun/HX711-Load-Cell-Amplifier/tree/38228f8f6602e1349d7130c7a382a436ceaca26e).
The exact qualification sources are:

| Upstream artifact | SHA-256 |
| --- | --- |
| [`hardware/SparkFun_HX711_Load_Cell.sch`](https://github.com/sparkfun/HX711-Load-Cell-Amplifier/blob/38228f8f6602e1349d7130c7a382a436ceaca26e/hardware/SparkFun_HX711_Load_Cell.sch) | `9eda665c31cf007e8e8d94b3a75c601522415a7624cc55ef63657fb3b0a4ada0` |
| [`hardware/SparkFun_HX711_Load_Cell.brd`](https://github.com/sparkfun/HX711-Load-Cell-Amplifier/blob/38228f8f6602e1349d7130c7a382a436ceaca26e/hardware/SparkFun_HX711_Load_Cell.brd) | `5bf12f6c208aa73b75ffcc8b6375914571a5aae857508a4289bfa28a7bbb0a5b` |

The upstream repository describes the design as open-source hardware and
public domain under its Beerware notice. CircuitC must retain that notice,
the upstream URL and commit, the two exact source digests, and a clear record
of the transcription and intentional differences. SparkFun names may be used
for attribution and factual identification only. SparkFun logos, decorative
copper, trademarks, panelization, fiducials that are not required by the
single-board product, and firmware are outside the reproduction.
The repository-level notice does not by itself resolve any separately marked
license on embedded library assets. Implementation must inventory each reused
source or geometry asset and either retain its compatible notice or reauthor
the CircuitC asset from an authoritative component or package specification.

This board is the smallest candidate that usefully crosses the current
bootstrap boundary. Its one-sheet functional design contains the HX711 in
SO16, a PNP transistor, resistors, capacitors, an inductor, a default-closed
solder jumper, and three bare plated-through connector patterns. The reference
outline is an exact 30.48 mm by 22.86 mm rectangle. The functional schematic
has 19 physical elements and 17 nets; the two-layer board uses five vias and
front and back ground pours. It therefore exercises real component, pin,
drill, via, zone, product, and fabrication semantics without requiring a
microcontroller, firmware, RF, high-speed routing, or a mixed-signal HX711
simulation model.

## Authority and fidelity boundary

The pinned Eagle bytes and their human-audited reference manifest own only the
qualification comparison:

- functional component roles, references, logical pins, and net connectivity;
- external connector signals and the default-closed 10-sample-per-second
  `RATE` configuration;
- the rectangular outline, board dimensions, two copper layers, placement
  intent, plated-through connector inventory, five-via inventory, and ground
  pours; and
- which differences from the upstream design are intentional.

CircuitC source owns the reproduced component identities, exact selected
manufacturer parts, source-authored schematic placement, board placement,
copper, product variant, and manufacturability intent. CircuitC does not claim
SparkFun's private production BOM, supplier choices, panel, certification, or
trademarked appearance. The qualification manifest is test evidence and may
not be consulted by normal compilation to add or repair missing intent.

The upstream Eagle layout is not expected to reproduce byte-for-byte as
KiCad. UUIDs, text placement, footprint artwork, and route geometry may differ
when the functional and bounded mechanical contract remains satisfied.
CircuitC's generated KiCad project, normalized fabrication inventory, and
host reports remain the only acceptance evidence for CircuitC output.

## Requirements

| ID | Requirement |
| --- | --- |
| `CC-REQ-QUAL-001` | The exact upstream repository, commit, source paths, SHA-256 digests, license notice, reference facts, and every intentional deviation are retained as reviewable offline provenance. A source digest or reference-manifest mismatch fails deterministically. |
| `CC-REQ-QUAL-002` | The source language and active Design IR v1 express physical components independently of simulator primitive kind, with arbitrary bounded logical-pin cardinality, an explicit no-value state for parts without a scalar value, and exact resistance, capacitance, and inductance values where applicable. Unsupported value/function pairs fail with stable source and IR diagnostics. |
| `CC-REQ-QUAL-003` | Canonical intent distinguishes procured components from electrically connected, non-procured fabricated board features. The three bare connector patterns and default-closed solder jumper retain pins, nets, pads, drills, placement, identity, and host parity without inventing manufacturer parts or entering catalog resolution, BOM quantity, or assembly population. |
| `CC-REQ-QUAL-004` | A reviewed vendored KiCad catalog and strict offline product snapshot cover every procured HX711 reference component, including the SO16 HX711, SOT-23 PNP transistor, 0603 and 1210 passives, and 0805 inductor. Every selected procured part has an exact CircuitC-authored manufacturer identity, part number, package, lifecycle, sourcing policy, symbol, footprint, and pin mapping. Fabricated board features use separately owned reviewed backend assets. |
| `CC-REQ-QUAL-005` | Canonical physical intent represents the exact rectangular outline, two outer copper layers, front- and back-side orthogonal placement, surface-mount pads, plated-through pads and drills, through-layer vias, source-authored track segments, a default-closed solder-jumper bridge, and bounded front/back ground zones using signed integer nanometres and stable semantic identities. Unsupported pad, drill, via, or zone forms fail before emission. |
| `CC-REQ-QUAL-006` | The CircuitC reference source matches the complete audited functional manifest bidirectionally: no component or fabricated feature, logical pin, external signal, internal net, connection, no-connect, configured jumper state, footprint, or required physical primitive is missing or added. Declaration permutations produce equal canonical Design IR and byte-identical artifacts. |
| `CC-REQ-QUAL-007` | KiCad lowering emits a complete isolated schematic, two-layer PCB, project, identity map, and design-derived library set. KiCad 10 parses every artifact, schematic-to-PCB parity is exact, and structured ERC, DRC, unconnected, and connectivity policies accept the generated reference board without relying on user-global configuration or allowlisting a product defect. |
| `CC-REQ-QUAL-008` | One explicit fully fitted reference variant resolves every procured component against checksum-pinned offline catalog evidence and produces deterministic, independently reverified product-resolution, BOM, placement, and assembly artifacts. Fabricated board features remain in PCB and host-position evidence but are excluded explicitly from procurement and assembly quantities. The default closed `RATE` configuration is authored and cannot be inferred from generated copper. |
| `CC-REQ-QUAL-009` | The KiCad 10.0.5 fabrication boundary accepts and normalizes the exact nonempty plated-through pad and via drill inventory, rejects missing, extra, wrong-plating, wrong-tool, wrong-diameter, and unsupported slot evidence, and reconciles every host position and drill back to current Design and product intent. Empty NPTH output remains valid only when the Design declares no non-plated holes. |
| `CC-REQ-QUAL-010` | Board analysis independently proves clean ERC, DRC, unconnected, schematic parity, and complete fabrication inventory over the same exact compiler artifacts and nonempty drill evidence. A successful process exit, a matching reference manifest, or any one green capability cannot substitute for another. |
| `CC-REQ-QUAL-011` | An independently verified content-addressed release closes the exact CircuitC source, complete Design identity, reference provenance and deviation record, catalog, product bundle, KiCad project, normalized fabrication files, analysis evidence, and exact applicable tools. Identical clean-checkout inputs reproduce byte-identical normalized release bytes offline. |

## Required architecture records

Implementation starts by recording the decisions that M5 intentionally leaves
open. One or more accepted ADRs must define:

- the general component, optional exact-value, and procured-versus-fabricated
  feature model across source, Design identity, product, and release contracts;
- exact pad-stack, plated-drill, via, and zone intent plus ownership of KiCad
  zone filling and rejection of stale or unowned fill caches;
- evolution of the strict product and fabrication v1 contracts for no-value
  identities, fabricated features, and nonempty drill inventories; and
- the retained third-party reference bytes, license/attribution boundary,
  audited manifest, and independent fidelity-verification contract.

These decisions may evolve the active unreleased Design IR v1 in place under
ADR-0003. They may not silently redefine authority in implementation code.

## Initial capability boundary

### General components and exact values

The pre-release source and Design IR evolve in place at version 1. A physical
component is no longer forced to masquerade as a resistor or independent DC
source. It has a logical function, arbitrary bounded logical pins, an explicit
symbol and optional footprint, explicit connection state for every pin, and
an optional simulator model. Simulation remains a separate capability.

The scalar value is explicit and closed: resistance, capacitance, inductance,
or no scalar value. Exact quantities remain decimal and dimensional. The
product-catalog and product-artifact contracts evolve coherently so that a
no-value identity cannot be matched to a valued part and incompatible value
kinds cannot be substituted. This epic does not add approximate string values
or an open-ended property bag.

### Vendored product catalog

The reference product uses reviewed CircuitC-owned catalog entries, not
display names extracted from Eagle or live KiCad libraries. Symbols,
footprints, pad stacks, silkscreen, fabrication graphics, and courtyards are
vendored deterministic backend assets with design-derived publication.
Procured components and fabricated board features are distinct: the latter
retain electrical and physical meaning but never acquire invented distributor
identity merely to satisfy a BOM schema.

The selected manufacturer identities and sourcing observations are explicit
CircuitC product choices. The upstream reference proves topology and bounded
mechanical fidelity; it does not authorize invented SparkFun procurement
evidence. Any part or footprint that intentionally differs from upstream is
named in the deviation record and must retain equivalent pin, package, and
board-interface semantics.

### Two-layer copper and drilled features

M5 adds only the physical forms required by the pinned board: surface-mount
and plated-through round or oblong pad stacks, round plated vias spanning the
two outer copper layers, straight source-authored tracks, a rectangular
outline, and polygonal ground zones on the front and back layers. Drill
diameters, pad sizes, clearances, zone boundaries, and coordinates remain
exact integer nanometres. KiCad owns fill, connectivity, and DRC acceptance;
generated zone fill is not canonical CircuitC intent.

The reference layout is source-authored canonical copper. It declares no
`autoroute` request, so APGAR evidence is inapplicable and forbidden from its
release. This epic does not weaken or widen the existing APGAR process
contract. The new via and zone forms must nevertheless be authenticated by
the same complete Design identity and checked against the complete emitted
KiCad copper inventory so unowned copper cannot enter accepted output.

### Qualification and release

A checked-in reference manifest records the bounded upstream facts and source
digests. An independent verifier compares that complete manifest with the
freshly elaborated CircuitC design in both directions. This proof establishes
fidelity only; the ordinary compiler, product, fabrication, analysis, and
release verifiers independently rederive their outputs from current CircuitC
source and pinned evidence.

The fully fitted default variant fits every procured component and declares
the `RATE` bridge closed for 10 SPS. Fabricated terminal patterns and jumper
copper are always-present board features, not fitted purchasable parts. No
second variant is implied. KiCad 10.0.5 must export nonempty PTH drill data for
connector pads and vias, while the absence of authored non-plated holes must
yield the exact accepted empty NPTH form. Drill parsing and normalization
become exact inventory evidence rather than the M4 zero-hit special case.

The release has neither simulation nor APGAR applicability because the source
declares neither analysis nor autoroute intent. It must not fabricate either
evidence chain merely to make the release appear more complete.

## Non-goals

- A general Eagle importer, Eagle-to-KiCad converter, or round-trip editor.
- Byte-identical KiCad output, route geometry, artwork, UUIDs, or production
  panelization relative to the upstream Eagle files.
- SparkFun logos, trademarked board appearance, or an assertion that SparkFun
  endorses the CircuitC reproduction.
- Reconstructing or claiming SparkFun's exact production BOM, suppliers,
  certifications, test process, or manufacturing yield.
- An Ohmnivore model for the HX711, transistor, load cell, or complete mixed-
  signal board.
- APGAR autorouting of the complete board, multipin routing, or GPU routing.
- Arbitrary board outlines, blind or buried vias, slots, castellations,
  controlled impedance, more than two copper layers, or unrestricted zone
  rules.
- A stable public CircuitC language or Design IR release.
- Ordering, assembling, calibrating, or electrically characterizing a physical
  board; those require a separately authorized hardware campaign.

## Acceptance gates

- The retained upstream schematic and board bytes match the two pinned
  SHA-256 digests, the license and attribution record is complete, and the
  audited reference manifest covers the entire bounded fidelity surface.
- The reference manifest verifier rejects a removed or extra component, pin,
  net, connection, no-connect, external signal, jumper state, footprint,
  placement, drill, via, zone, or intentional-deviation entry.
- Source-order permutations and equivalent exact quantity spellings elaborate
  to equal Design IR and produce byte-identical compiler, product, and release
  artifacts in independent processes.
- Focused source, IR, catalog, KiCad, product, fabrication, analysis, and
  release tests each include a mutant that removes their new guard or emitted
  behavior and fails without relying on the integrated fixture alone.
- KiCad 10.0.5 parses the complete isolated project and returns normalized
  structured ERC and DRC evidence with clean connectivity, no unconnected
  items, exact schematic parity, and no product-defect allowlist.
- Repeated KiCad fabrication exports normalize to byte-identical Gerber,
  Gerber-job, Excellon, position, request, manifest, analysis, and release
  bytes. The parsed PTH tool and hit inventory equals the current Design's
  connector-pad and via drills bidirectionally, and NPTH remains empty.
- Product resolution, BOM, placement, and assembly reconcile every fitted
  procured component against the current Design, selected variant, and exact
  offline catalog snapshot with no orphaned or coordinated extra row, while
  host evidence retains every explicitly non-procured fabricated feature.
- A clean checkout reproduces and independently verifies the complete
  content-addressed default-variant release without network access or
  user-global KiCad state.
- All required Bazel lint, ordinary and strict-lockfile build/test, module
  graph, supported-host KiCad, fabrication, board-analysis, and release gates
  pass on one exact candidate head. Any unavailable host gate is recorded as
  unavailable and blocks completion.
