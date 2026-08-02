# EPIC-0002: Useful KiCad project compiler

- Status: Complete
- Architecture milestone: M1B
- Depends on: EPIC-0001

## Outcome

A CircuitC design compiles offline and deterministically into a complete KiCad
10 project containing an electrically meaningful schematic, a corresponding
PCB, and isolated project configuration accepted by KiCad ERC and DRC.

## Requirements

| ID | Requirement |
| --- | --- |
| `CC-REQ-KICAD-001` | The language and Design IR represent hierarchy, typed interfaces, electrical pin types, and explicit connected and no-connect states. |
| `CC-REQ-KICAD-002` | Manufacturer identity, logical device, symbol, footprint, pad, and model bindings are explicit and validated rather than inferred from display names. |
| `CC-REQ-KICAD-003` | Required KiCad symbols, footprints, models, and configuration are vendored or checksum-pinned and rebuild without network access or user-global KiCad configuration. |
| `CC-REQ-KICAD-004` | CircuitC emits deterministic `.kicad_sch`, `.kicad_pcb`, `.kicad_pro`, and required library-table artifacts from canonical intent. |
| `CC-REQ-KICAD-005` | Placement and route authoring generalize beyond the M1A fixture while preserving exact nanometre coordinates and semantic identities. |
| `CC-REQ-KICAD-006` | KiCad findings map back to CircuitC semantic identities and source locations through normalized structured reports. |
| `CC-REQ-KICAD-007` | A clean checkout passes supported KiCad 10 parsing, ERC, DRC, connectivity, and schematic-parity gates without relying on manual editor state. |

## Non-goals

- A CircuitC GUI or KiCad plugin.
- Silent preservation of arbitrary edits made only to generated KiCad files.
- APGAR route search.
- Broad manufacturing release management.

## Completed vertical slice

The reference voltage divider expands M1A through one canonical lowering:
EPIC-0002 corrects the unreleased M1A-era orthogonal-rotation handedness so a
90-degree placement maps `(x, y)` to `(y, -x)` in KiCad's Y-down frame.

```text
CircuitC source
  -> module-instance hierarchy and typed ports
  -> explicit part, symbol-pin, footprint-pad, and simulator-model bindings
  -> canonical Design IR with connected/no-connect pin states
  -> deterministic KiCad project bundle
       voltage_divider.kicad_sch
       voltage_divider.kicad_pcb
       voltage_divider.kicad_pro
       sym-lib-table / fp-lib-table
       CircuitC.kicad_sym / CircuitC.pretty/*.kicad_mod
       voltage_divider.kicad-map.json
  -> isolated KiCad 10 ERC, DRC, connectivity, and schematic-parity policy
```

Module paths describe elaborated instances rather than reusable source
templates in this milestone. A dotted child module requires its parent, every
component belongs to the module named by the parent of its semantic path, and
ports explicitly carry direction, electrical pin type, and connected or
no-connect state. M1B backends use the instance tree for component ownership;
typed module-port direction, electrical type, and state are validated but are
not yet lowered to KiCad or SPICE because ports add no connectivity beyond the
existing nets they reference. Reusable parameterized module definitions remain
compatible with this instance-tree IR but are not required to prove M1B.

The initial vendored library catalog deliberately contains only the symbols
and footprints exercised by the vertical slice. Source bindings are resolved
against that catalog during elaboration; an unknown symbol, footprint, pin, or
pad is a source diagnostic. Extending catalog coverage is additive and does
not give KiCad library display names authority over CircuitC identities. The
compiled bundle derives a sorted vendored-file set from the exact symbols and
footprints used by the design, so adding a catalog footprint cannot publish an
incomplete project bundle.

The schematic is a deterministic projection of canonical connectivity. It is
laid out from exact source-authored schematic positions, embeds the resolved
symbol definitions, labels every connected pin with its canonical net, and
places a KiCad no-connect marker on every explicit no-connect pin. PCB
footprint paths use the corresponding schematic symbol UUID. Physical
no-connect pads receive deterministic KiCad-only unconnected nets, while the
canonical Design IR remains unnetted, so KiCad's parity checker is an authority
rather than a separate heuristic.
Schematic anchors are unique, and the backend rejects transformed symbol-pin
connection points that coincide with a different canonical connection state.

The generated identity map joins every emitted KiCad UUID to a CircuitC
semantic path and, for source compilation, its UTF-8 source span. Its exact,
fail-closed wire contract and pre-release compatibility policy are defined by
[`schemas/kicad_identity_map/v1.md`](../../schemas/kicad_identity_map/v1.md).
Normalized host reports use this map to retain stable source ownership while
removing host paths, timestamps, and finding order.

## Acceptance gates

- Repeat builds produce byte-identical project artifacts under an identical
  pinned toolchain.
- KiCad 10 parses the generated schematic, PCB, symbol, and footprint
  artifacts; CircuitC validates the exact generated project-JSON subset because
  `kicad-cli` has no direct `.kicad_pro` parser.
- Structured ERC and DRC contain no unexpected findings or unconnected items.
- Schematic-to-PCB parity is clean.
- Tests run from an isolated KiCad configuration in a clean checkout.
- All repository Bazel build, test, formatting, lint, and lockfile gates pass.

## Completion evidence

Published for review on 2026-08-01 as PR #2. The initial implementation commit
is `592b9d77c0438135297496ca6a9acde6615ace69`, based directly on repository
commit `d93c66175fa2b912903496532b029dff496fdbf9`; that identity is historical, not
the current review head. Each review-fix head is bound to its exact OID in the
PR check and review records. The OID is intentionally not embedded in this
tracked file because changing the file would itself change that OID. The full
gate matrix below is rerun before each review-fix head is pushed, and the digest
block below was regenerated from the current source tree during that final gate
run.

| Requirement | Implementation ownership | Tests and completion evidence |
| --- | --- | --- |
| `CC-REQ-KICAD-001` | `src/design.rs`, `src/frontend/{syntax,parser,elaborate}.rs`, `schemas/design_ir/v1.md`, and `docs/language.md` | Module/port hierarchy, electrical types, connection-state validation, optional simulation bindings, missing-parent rejection, declaration permutations, source/Rust IR equivalence, and the source-authored fixture containing a physical-only/no-connect component pass in `//:circuitc_test`. Success coverage includes an `inout` module port with an explicit no-connect state through Design IR validation. Component module ownership is derived from the parent of its semantic path instead of stored redundantly. A ground-less physical-only source compiles, while adding a simulator model restores the single-ground diagnostic; assigning no-connect to a retained simulation terminal produces `CC-SIM-003` at both the Design and compile seams without reaching a SPICE panic. Failure-side tests pin every new fail-closed source diagnostic's code, message, and primary span, exercise recovery after unsupported module declarations, and retain related spans for duplicates. |
| `CC-REQ-KICAD-002` | `src/library.rs`, compiler catalog validation, explicit part/symbol/footprint/model syntax, and vendored assets under `libraries/` | Full logical-device/manufacturer/MPN tuples bind one coherent symbol/footprint definition. Unknown or incoherent tuples, symbols, footprints, pins, pad geometry, and models produce stable diagnostics; public-IR adversarial tests confirm failures do not panic. The symbol validator and emitter share exact vendored-definition extraction, with missing-file and missing-definition cases pinned to `CC-KICAD-SYMBOL-007`. |
| `CC-REQ-KICAD-003` | Bazel `compile_data`, `libraries/CircuitC.kicad_sym`, `libraries/CircuitC.pretty/`, and generated `${KIPRJMOD}` library tables | Static catalog definitions expose footprint geometry, drawing geometry, typed table metadata, and publishable files without constructing discarded owned footprints; `CompiledArtifacts` carries the sorted library files selected by the design. Consistency tests tie each symbol pin number to its exact catalog offset, pin the complete populated table structure and URI, and pin emitted-board silkscreen/courtyard geometry together with each graphic's identity UUID; the host gate exports the symbol and footprint with isolated configuration and no user-global dependency. |
| `CC-REQ-KICAD-004` | `src/kicad.rs`, `CompiledArtifacts`, `//cmd/circuitc`, and the Rust fixture generator | Full-bundle CLI tests compare source builds across filenames, including identity maps, and against the Rust oracle; the default test also validates the emitted project JSON and both library tables, including design-derived `.kicad_pro` metadata for a second design name. Component-specific tests pin exact source values in both schematic and PCB artifacts, including a physical-only part and a mutated value. Schematic tests pin `in_bom`/`on_board` for both physical and virtual symbols, while board tests prove the footprint `sheetfile` follows two distinct design names. Unit and process-boundary tests pin unsafe generated-path rejection before root creation, symlinked-ancestor and target rejection, write-only regular-target replacement through directory authority, the documented error for fully inaccessible targets, transactional containment, exit status, actionable diagnostics, a racing destination that the real no-replace syscall preserves without staging residue, and checked staging-file synchronization whose injected failure preserves originals and removes temporary files. The host gate compares independent source builds byte for byte. |
| `CC-REQ-KICAD-005` | Exact schematic/board placement and explicit route lowering in the language, Design IR, and KiCad backend | Full-turn and declaration-order tests preserve canonical IR and artifacts; route UUIDs remain stable when geometry moves, and all coordinates lower from integer nanometres without floating point. Canonical anchor and transformed pin-point collision tests reject schematic placements that could merge distinct connection states or merge distinct no-connect pins. The schema and language fix counterclockwise rotation in KiCad's Y-down frame; asymmetric unit tests pin every orthogonal board transform, back-layer tests pin pad, silkscreen, and courtyard layers, and a KiCad-host fixture cross-checks schematic rotation with a physical-only resistor whose pins have different connected and no-connect states. |
| `CC-REQ-KICAD-006` | Deterministic `KicadIdentity` enumeration, `<design>.kicad-map.json`, [`schemas/kicad_identity_map/v1.md`](../../schemas/kicad_identity_map/v1.md), and `tools/kicad/normalize_drc.py` | Tests assert exact set equality between every schematic/board UUID and the identity map for both reference fixtures, plus UUID and semantic-path uniqueness. Global rendered-semantic-path collision tests include source primary/related spans for structural, footprint, and footprint-graphic identities. Derived connection diagnostics resolve to their owning component; an exact route identity retains the route's source location while a route whose path is only a generated-identity prefix cannot capture derived component provenance. The full-bundle CLI test feeds its freshly generated map into the strict ERC normalizer. Schema/type/exact-field/source/UUID/path/missing/duplicate/location manifest tests, missing and unknown finding-UUID rejection, exact severity and ignored-check policy, and multi-finding source-correlated ERC/DRC/unconnected/parity aggregation pass under Bazel. |
| `CC-REQ-KICAD-007` | `//:kicad10_drc_test`, `//:kicad_project_validator_test` | KiCad 10.0.5 parsed/exported the vendored libraries and ran isolated ERC/DRC/parity for both the voltage divider and the source fixture containing a rotated physical-only/no-connect component. Each reported 0 ERC violations, 0 DRC violations, 0 unconnected items, and 0 parity issues. The deterministic project validator accepts both generated project files and rejects malformed JSON, invalid or nonempty nested structure, and filename mismatch. |

Exact successful gates from the published implementation and repeated on each
review-fix head:

```text
bazel lint //...
bazel build //...
bazel test //...
bazel test --lockfile_mode=error //...
bazel mod graph --lockfile_mode=error
bazel test //:kicad10_drc_test --nocache_test_results --test_output=errors
```

The host gate produced identical normalized project, ERC, and DRC evidence for
two independent builds of each source fixture. The rotated fixture proves that
a physical-only resistor's connected pad and explicit no-connect pad survive
schematic-to-PCB parity with the correct pin identities. This gate is tagged
`local` and `manual`; the recorded host
evidence was produced on a local macOS host with KiCad 10.0.5 and is not
reproduced by Linux CI. The generated reference bundle has these SHA-256
values:

```text
271039f04a30790249b2a59e1df4ce3324cc19e4d7936d466c8b8ea0ed32e707  voltage_divider.kicad_sch
f6f1ea637252f58f83d238a88193dc39d0d56e339be7208327ddd16185ead465  voltage_divider.kicad_pcb
201bc75180ca7d38f797023bf001f2d39575f281d2440cae505bdf382a39a7a7  voltage_divider.kicad_pro
6b853363a4daefffb57c6ba13a51d5592fe6e7e1ba93a5f058f6e17858223633  CircuitC.kicad_sym
47b232a8ed3191d6055a4f8760b0f374ad770062674ee59bac9187a4934868bf  CircuitC.pretty/R_0603_1608Metric.kicad_mod
b3c0b7098fe43935a8fd3942b85261f17462ca069a2f0aa76cef599db9b26d22  sym-lib-table
080eb955b8d15f67e9f6ee383a1a3707aec6fdc41bfb8b5dc8a0d0a8d9a1fdc2  fp-lib-table
95915e68534f772439eca8e47d55a0611d2416369e6cd9ecbc2590523cc655bf  voltage_divider.kicad-map.json
43a5f70c8f1e4bbdf428027a1b88e450f02ea6eacf9015f2cd953d65b174c0a8  voltage_divider.spice
```
