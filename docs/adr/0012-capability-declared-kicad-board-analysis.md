# ADR-0012: Bind distinct KiCad board-analysis capabilities to exact host evidence

- Status: Accepted
- Date: 2026-08-04

## Context

ADR-0011 authenticates deterministic fabrication output and proves the exact
generated PCB has a complete manufacturing-file inventory. It deliberately
does not evaluate the authored `erc_clean`, `drc_clean`, `unconnected_clean`,
or `schematic_parity_clean` assertions. The older generic KiCad host gate runs
ERC and DRC, but a successful process exit or one aggregate "clean" flag
cannot prove which authored capability was evaluated, which exact board and
schematic were used, or whether fabrication evidence belonged to the same
Design.

The first product-analysis adapter therefore needs a versioned request,
structured execution result, and assertion report. It must retain KiCad as the
authority for KiCad output while keeping CircuitC source, Design IR, product
resolution, and the Layer-4 fabrication manifest authoritative at their own
boundaries.

## Decision

Layer 5 adds the strict
`circuitc.board_analysis_request`, `circuitc.board_analysis_result`, and
`circuitc.board_analysis_report` v1 contracts. The public Rust boundary
consumes:

- one valid Design and the exact authored `kicad` version `10` analysis;
- the exact catalog snapshot, selected variant, Layer-3 product bundle, and
  static or opaque checked compiler evidence required by ADR-0011;
- one opaque Layer-4 `FabricationManifestBundle`, whose exact request is
  independently reconstructed from those current inputs;
- the exact compiler-emitted schematic, PCB, and frontend KiCad identity-map
  bytes; and
- an execution receipt plus exact normalized ERC and DRC report bytes.

Board-analysis v1 requires exactly one assertion for each of
`erc_clean`, `drc_clean`, `unconnected_clean`,
`schematic_parity_clean`, and `fabrication_inventory_complete`. The request
orders them by that closed capability order, independent of authored
declaration order. Missing, duplicated, additional, or substituted capability
sets are unsupported rather than weakened.

The normal compiler remains host-free. Analysis is a separate post-compile
boundary entered through Bazel.

## Request identity and fixed policy

`analysis_identity_sha256` is lowercase SHA-256 over ASCII
`CIRCUITC-BOARD-ANALYSIS-IDENTITY-V1`, one NUL byte, and one compact canonical
JSON preimage without a final LF. In fixed field order the preimage contains:

- Design name, analysis path, adapter, expected major, and exact expected host
  version;
- all five assertion paths and capabilities;
- exact path, byte length, and SHA-256 bindings for the schematic, PCB, KiCad
  identity map, Layer-4 fabrication request, and Layer-4 fabrication manifest;
- the exact expected ERC sheet inventory derived from the authenticated
  compiler-emitted schematic identity;
- the complete exact compiler-emitted KiCad project-support inventory: project
  file, library tables, symbol libraries, and footprint-library files;
- the complete ERC/DRC finding policy and resource policy; and
- the exact host output inventory.

The root is `board-analysis/<analysis_identity_sha256>/`. Request, result, and
report occupy `request.json`, `result.json`, and `report.json`. Completed
normalized evidence occupies `evidence/erc.normalized.json` and
`evidence/drc.normalized.json`.

KiCad version is exactly `10.0.5`. Both reports require severities `error`,
`exclusion`, and `warning`. ERC permits only the exact ignored checks
`footprint_filter`, `four_way_junction`, `simulation_model_issue`, and
`single_global_label`. DRC permits only `footprint_filters_mismatch`,
`footprint_type_mismatch`, `missing_courtyard`,
`track_not_centered_on_via`, and `tuning_profile_track_geometries`. The only
accepted DRC finding is KiCad's exact missing local `CircuitC` footprint-library
warning, with every item joined through the exact identity map. No electrical,
clearance, connectivity, or parity finding is allowlisted.

## Distinct capability evaluation

The normalized ERC report independently establishes `erc_clean`: its ordered
sheet path and UUID-path inventory exactly equals the request-bound inventory,
and every sheet has an empty violation list. Missing, duplicate, reordered, or
substituted sheet evidence is invalid rather than a failed clean assertion.

The normalized DRC report establishes three separate facts:

- `drc_clean` accepts no finding except the exact environmental library warning
  above;
- `unconnected_clean` requires the structured `unconnected_items` inventory to
  be empty; and
- `schematic_parity_clean` requires the structured `schematic_parity`
  inventory to be empty.

The opaque, current-input-authenticated Layer-4 predecessor independently
establishes `fabrication_inventory_complete`. No one green field substitutes
for another. The report contains exactly one outcome per authored assertion
and names its evidence role.

## Result and report states

Every result has one exact execution status: `completed`, `failed`, or
`unsupported`.

- `completed` contains the exact KiCad executable, committed normalizer, and
  committed host-runner digests plus a complete indivisible ERC, DRC, and
  fabrication evidence set; it has no diagnostic. Structurally valid findings
  remain completed evidence and produce capability-local `fail` outcomes.
- `failed` and `unsupported` contain no tool or partial evidence object and
  carry one bounded canonical diagnostic.

Every report repeats that status and contains all five assertion outcomes.
Completed evidence yields an independently evaluated `pass` or `fail` outcome
for every capability. A failed execution yields five `unevaluated` outcomes. An
unsupported execution yields five `unsupported` outcomes. Only a completed
report whose five outcomes all pass sets `all_pass` true. Later release binding
must recompute and require that condition; callers cannot turn dirty or
noncompletion evidence into acceptance.

## Host execution and receipt

The request drives a fixed runner that validates its complete canonical bytes
and identity before execution. It authenticates the exact schematic, PCB,
identity map, fabrication manifest, KiCad executable, committed normalizer, and
host-runner bytes with bounded single-link no-follow reads through anchored
directory components. Each tool executes from its authenticated immutable
snapshot with isolated Python startup and an explicit minimal environment. The
host runner constructs a private closed project from the authenticated
schematic, PCB, identity-map, project, table, symbol-library, and
footprint-library bytes. Ambient design-rules and caller-project files never
enter the execution namespace.
It binds each normalized report to its pre-execution source digest and rechecks
the staged source after the host and normalizer return.

Every protected work or publication path is opened component-by-component
without following symlinks. Every ancestor must be owned by root or the
effective UID; the terminal directory must be owned by the effective UID and
must not be group- or other-writable. Held descriptors and directory identities
are rechecked across process execution, cleanup, and no-replace publication.

The Layer-5 runner bounds process lifetime and stdout/stderr, rechecks every
input identity and digest after both host operations, and publishes exactly
`erc.normalized.json`, `drc.normalized.json`, and `receipt.json` with one atomic
no-replace directory rename. The canonical receipt binds the exact request,
schematic, PCB, identity map, executable, normalizer, host runner, and both
normalized reports. The Rust binder recomputes every receipt digest before
emitting a completed result.

The caller's UID, installed KiCad bundle resources, and the committed
normalizer/runner implementation remain in the local trusted computing base.
The normalized reports are host evidence, not canonical Design IR.

## Bounds and determinism

Each request, result, report, receipt, identity map, raw or normalized host
report, tool image, and predecessor binding is at most 67,108,864 bytes. Report
primary rows are limited to 10,000 and retained diagnostics to 256. The checked
aggregate of all consumed analysis inputs and the checked aggregate of emitted
request/result/report/evidence bytes are each limited to 268,435,456 bytes.
Limits fail without a partial accepted bundle.

Request, result, and report are compact canonical JSON plus one LF. Normalized
host reports use the existing sorted two-space KiCad evidence form and must
round-trip byte-exactly. Duplicate keys, reordered contract bytes, unknown
fields, stale digests, wrong host or policy, missing or additional capability
outcomes, and coordinated bundle rewrites fail closed.

## Consequences

- KiCad ERC, DRC, unconnected, schematic parity, and fabrication completeness
  remain distinct acceptance authorities with explicit Design-owned requests.
- A successful process exit is necessary but never sufficient.
- Layer-4 fabrication evidence is consumed as an authenticated predecessor; it
  is not reinterpreted as analysis-owned placement or product truth.
- Failed and unsupported attempts are representable deterministic evidence but
  cannot become release acceptance.
- Layer 5 does not bind source identity, simulation/routing acceptance, the
  complete release inventory, or transactional release publication. Those
  remain Layer 6.
