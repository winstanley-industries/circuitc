# ADR-0002: KiCad 10 files are deterministic generated artifacts

- Status: Accepted
- Date: 2026-08-01

## Context

CircuitC is code-oriented and has no UI. KiCad is the first output target and
provides mature viewing, fabrication export, ERC, and DRC. CircuitC needs a
clear answer for source authority, manual editor changes, headless validation,
and file-format versioning.

KiCad documents its s-expression schematic and board formats. `kicad-cli`
provides headless ERC and DRC. The KiCad IPC API is language-neutral, but KiCad
9 and 10 require a running GUI and its initial coverage is PCB-focused, so it
is not the primary compiler boundary.

## Decision

CircuitC source and canonical Design IR own connectivity and physical intent.
KiCad project files are deterministic outputs and are regenerated from that
intent. CircuitC does not attempt a silent three-way merge with arbitrary
manual edits.

The first backend slice emits the KiCad 10 board format stamped `20260206`,
identifies the generator as `circuitc`, and uses exact nanometre-to-millimetre
decimal conversion. The backend is not complete until KiCad 10 can parse the
artifact and emit structured DRC output.

KiCad schematic and project emission follow in M1. Until then, the M0 board
artifact demonstrates only placement, pads, nets, an outline, and route
segments.

## Consequences

- Builds are reproducible and generated files are safe to compare byte for
  byte.
- All durable layout and routing changes must be representable in CircuitC.
- Importing existing KiCad work is a separate adapter and transcription
  workflow.
- A new KiCad major version requires an explicit backend compatibility change,
  host validation, and golden updates.

## References

- [KiCad s-expression format](https://dev-docs.kicad.org/en/file-formats/sexpr-intro/index.html)
- [KiCad board file format](https://dev-docs.kicad.org/en/file-formats/sexpr-pcb/)
- [KiCad command-line interface](https://docs.kicad.org/10.0/en/cli/cli.html)
- [KiCad IPC API limitations for add-on developers](https://dev-docs.kicad.org/en/apis-and-binding/ipc-api/for-addon-developers/)
