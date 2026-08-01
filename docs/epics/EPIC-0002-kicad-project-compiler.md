# EPIC-0002: Useful KiCad project compiler

- Status: Planned
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

## Acceptance gates

- Repeat builds produce byte-identical project artifacts under an identical
  pinned toolchain.
- KiCad 10 parses every generated artifact.
- Structured ERC and DRC contain no unexpected findings or unconnected items.
- Schematic-to-PCB parity is clean.
- Tests run from an isolated KiCad configuration in a clean checkout.
- All repository Bazel build, test, formatting, lint, and lockfile gates pass.

## Completion evidence

Not yet complete.
