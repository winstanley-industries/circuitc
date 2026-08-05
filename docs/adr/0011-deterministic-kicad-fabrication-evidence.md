# ADR-0011: Normalize exact KiCad fabrication exports into a Design-bound manifest

- Status: Accepted
- Date: 2026-08-04

## Context

[ADR-0010](0010-deterministic-product-artifact-bundle.md) establishes one
exact product resolution, BOM, placement, and assembly bundle for a selected
variant. That bundle deliberately makes no KiCad or manufacturing-host claim.
The next layer must prove that the exact generated PCB can be exported by the
supported host and that the native fabrication inventory still agrees with
Design and product intent.

KiCad 10.0.5 is the first fabrication host. Its Gerber X2, Gerber-job, and
Excellon outputs embed the host wall clock. `SOURCE_DATE_EPOCH` does not affect
those fields. Therefore raw host bytes cannot honestly be release artifacts or
repeat-build evidence. KiCad's position CSV is stable, but it uses the host
coordinate convention and contains every board footprint rather than one
product variant's fitted subset.

The original generated PCB layer table also declared only copper, profile,
margin, and courtyard layers even though emitted footprints reference paste,
mask, silkscreen, and fabrication layers. KiCad silently omitted undeclared
manufacturing layers during export. Fabrication completeness therefore
requires a complete KiCad 10 layer table and exact native file-function checks,
not merely a successful process exit.

## Decision

Layer 4 adds a public fabrication request, binder, and verifier boundary. It
consumes:

- one valid Design IR value and the exact selected `kicad` major-version `10`
  manufacturability analysis and `fabrication_inventory_complete` assertion;
- exact pinned catalog snapshot bytes and one exact variant path;
- the exact Layer-3 product bundle, which is reverified from Design and catalog
  evidence;
- explicit compiler evidence: static Designs require independently recompiled
  and exactly compared `CompiledArtifacts`; Designs with simulation analyses
  or routing requests require opaque `CheckedCompiledArtifacts`; simulation
  boards are independently recompiled and routed boards are deterministically
  replayed from the current Design plus the retained authenticated route result
  and exactly compared; and
- an explicit path-to-bytes map containing the exact KiCad 10.0.5 native output
  inventory.

The normal compiler remains host-free. `compile` and `compile_checked` do not
invoke KiCad or select a product variant. Fabrication is a separate post-compile
boundary entered through Bazel.

## Fixed KiCad 10.0.5 profile

Fabrication v1 accepts exactly KiCad `10.0.5`. The binder receives the exact
executable bytes, computes their lowercase SHA-256 internally, and retains it
as execution evidence; it never accepts a caller-asserted executable digest.
The pre-execution request is validated before host launch and its fixed typed
profile derives host arguments. Callers cannot provide arbitrary command lines
or board plot settings.

Gerber export uses X2, millimetres, coordinate precision 6, net attributes,
page origin, and portable `.gbr` names. Protel extensions, saved board plot
parameters, DNP filtering, variants, zone mutation, and drill-origin changes
are disabled. The exact layer inventory, ordered by KiCad layer ID, is:

| ID | Layer | Native file function | Job file function |
| ---: | --- | --- | --- |
| 0 | `F.Cu` | `Copper,L1,Top` | `Copper,L1,Top` |
| 1 | `F.Mask` | `Soldermask,Top` | `SolderMask,Top` |
| 2 | `B.Cu` | `Copper,L2,Bot` | `Copper,L2,Bot` |
| 3 | `B.Mask` | `Soldermask,Bot` | `SolderMask,Bot` |
| 5 | `F.SilkS` | `Legend,Top` | `Legend,Top` |
| 7 | `B.SilkS` | `Legend,Bot` | `Legend,Bot` |
| 13 | `F.Paste` | `Paste,Top` | `SolderPaste,Top` |
| 15 | `B.Paste` | `Paste,Bot` | `SolderPaste,Bot` |
| 25 | `Edge.Cuts` | `Profile,NP` | `Profile` |

The generated PCB declares the complete standard KiCad 10 outer copper,
adhesive, paste, silkscreen, mask, user, profile, margin, courtyard, and
fabrication layer table. Only the nine layers above enter fabrication v1.

Drill export is Excellon, absolute origin, decimal metric coordinates,
alternate oval representation, and separate PTH/NPTH files. Maps, reports,
tenting output, mirroring, and minimal headers are disabled. Design IR v1 has
no hole or drill construct, so fabrication v1 accepts only the exact zero-tool,
zero-hit PTH and NPTH native forms. Any tool, round hit, slot, missing file, or
additional drill output fails as unsupported rather than being omitted.

Position export is both sides, CSV, millimetres, page origin, with no bottom-X
negation, SMD-only restriction, through-hole exclusion, DNP exclusion, or
KiCad variant. It deliberately includes every physical board footprint.

## Request identity and paths

`fabrication_identity_sha256` is lowercase SHA-256 over ASCII
`CIRCUITC-FABRICATION-IDENTITY-V1`, one NUL byte, and a compact canonical JSON
preimage without final LF. In fixed field order, the preimage covers:

- Design name and exact analysis, assertion, and variant paths;
- variant identity, product-input, exact resolution, and exact placement
  digests;
- the authored catalog evaluation date;
- exact PCB path and exact PCB byte digest;
- expected adapter, major, and exact version;
- the complete fixed export and resource profile; and
- the exact ordered role and relative-suffix output descriptors.

It does not cover output digests, executable path, filesystem metadata, raw
timestamps, host paths, or directory enumeration order. The root is
`fabrication/<fabrication_identity_sha256>/`. The request and manifest occupy
`request.json` and `manifest.json` under that root. The exact native and
normalized suffix inventory is:

```text
gerber/<design>-F_Cu.gbr
gerber/<design>-F_Mask.gbr
gerber/<design>-B_Cu.gbr
gerber/<design>-B_Mask.gbr
gerber/<design>-F_Silkscreen.gbr
gerber/<design>-B_Silkscreen.gbr
gerber/<design>-F_Paste.gbr
gerber/<design>-B_Paste.gbr
gerber/<design>-Edge_Cuts.gbr
gerber/<design>-job.gbrjob
drill/<design>-NPTH.drl
drill/<design>-PTH.drl
position/<design>-all-pos.csv
```

All paths use `RelativeArtifactPath`. Missing, extra, aliased, duplicate,
unsafe, or misnamed paths fail before parsing.

## Narrow native normalization

Raw host files are transient and never enter the returned bundle. CircuitC
strictly validates native structure before normalization:

- every Gerber must match the exact ordered KiCad 10.0.5 header envelope and
  terminal `M02`, bind the exact project, native file function and polarity,
  X2 4.6 coordinates, and millimetres, and contain no relocated, concatenated,
  or additional controlled command;
- the Gerber job must bind the same host and project and its path, function,
  and polarity inventory must equal all nine Gerbers bidirectionally;
- both Excellon files must bind the exact host, plating class, framing, metric
  decimal absolute policy, and zero-hit v1 form; and
- position CSV must have the exact seven-column KiCad 10 header and bounded
  rows.

Only these recognized volatile fields are rewritten:

- Gerber `%TF.CreationDate` and the KiCad created-by comment;
- Gerber-job `Header.CreationDate`; and
- the native and X2 creation-date comments in each Excellon file.

They become the authenticated catalog evaluation date at `00:00:00Z`, or the
same local timestamp shape where the native comment has no zone. A missing,
duplicate, malformed, relocated, or additional time field fails. Every other
byte is preserved. The position CSV is not modified. The manifest binds only
these normalized native bytes.

## Position and population reconciliation

KiCad position `PosX` is converted exactly from six-decimal millimetres to
integer nanometres. `PosY` is negated during that exact checked conversion to
recover the Design coordinate convention. Rotation must be exactly 0, 90,
180, or 270 degrees; `top` and `bottom` map to Design `front` and `back`.
Floating point is never used.

Every unique host reference must join to exactly one physical Design
component. Host value equals the exact KiCad value lowering and host package
equals the exact footprint library-item name. Coordinates, side, and rotation
must equal Design. The manifest records the Layer-3 population state for each
row. Fitted and alternate rows are already proven present in canonical
placement by Layer-3 verification; not-fitted rows remain valid full-board host
parity evidence but contribute nothing to canonical product placement. KiCad
DNP state and CSV inventory never select product population or part identity.

## Manifest and verification

The strict `circuitc.fabrication_manifest` v1 binds the request bytes, exact
PCB, product roots, authored evaluation date, fabrication identity, fixed
profile, exact host version and executable digest, every normalized path,
length, SHA-256, parsed Gerber layer, parsed zero drill counts, and normalized
position rows. The binder recomputes all caller-supplied byte digests and
counts. A successful host exit is necessary but never sufficient.

The verifier reconstructs the request and complete normalized bundle from the
same authoritative Design, catalog, product, compiled board, exact executable
bytes, and explicit raw path-to-bytes inputs and requires exact bundle
equality. Rewriting manifest fields or normalized files, including a
coordinated rewrite, therefore fails against authoritative recomputation.

Each source, native output, request, manifest, and normalized output is at most
67,108,864 bytes. The exact host file count is 13, position rows are at most
10,000, and the complete request, manifest, and normalized output aggregate is
at most 268,435,456 bytes. Limits and checked aggregates fail without a partial
bundle.

## Host isolation

The supported host runner validates the canonical pre-execution request, then
snapshots only the explicitly named board and executable bytes through bounded
`O_NOFOLLOW` regular-file descriptors into a fresh mode-0700 transaction. It
holds and rechecks both staged identities before and after every host command,
uses private config, home, and temporary roots with a closed stdin and fixed
locale, enforces time and stdio bounds, opens only predetermined outputs
without following links, and uses directory listing only to reject extras. A
complete authenticated raw tree plus its transient receipt is published with
one atomic no-replace rename. The receipt binds the exact request, board,
executable, and every raw output digest. The gate opens the root and all child
directories component-by-component without following links, retains directory
and file descriptors across bounded reads, enforces the aggregate before
allocation, and rechecks the same descriptor namespace and receipt before
emitting the verified manifest. The documented same-UID host process and the
installed KiCad bundle resources loaded by the authenticated executable remain
in the local trusted computing base.

## Consequences

- The exact generated board is accepted by a real supported fabrication host.
- Host-clock differences cannot perturb released normalized bytes.
- Missing manufacturing layers, drill ambiguity, output extras, and position
  drift fail closed.
- CircuitC source, Design IR, and Layer-3 product artifacts retain authority;
  native host output cannot redefine placement or population.
- Simulation-only boards are reproduced without rerunning Ohmnivore; routed
  boards are reproduced by strict replay of opaque checked APGAR evidence. The
  later release layer still binds and publishes the full upstream evidence
  chain, but Layer 4 does not defer board authentication to it.
- Layer 4 does not evaluate ERC, DRC, unconnected, or parity assertions and
  does not close or publish a release. Those remain later layers.
