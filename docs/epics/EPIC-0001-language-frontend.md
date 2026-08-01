# EPIC-0001: CircuitC language frontend

- Status: Active
- Architecture milestone: M1A
- Depends on: EPIC-0000

## Outcome

A user can author the reference voltage divider in a minimal declarative
`.circuitc` file and invoke a headless Bazel-built CLI to produce the same
validated KiCad PCB and SPICE semantics as the programmatic bootstrap fixture.

## Requirements

| ID | Requirement |
| --- | --- |
| `CC-REQ-LANG-001` | CircuitC defines a deliberately small declarative syntax sufficient for the existing voltage-divider Design IR semantics. |
| `CC-REQ-LANG-002` | Lexing and parsing retain input identity and UTF-8 byte spans without placing syntax-tree details in the canonical Design IR. |
| `CC-REQ-LANG-003` | Decimal dimensions and electrical quantities lower exactly, without floating-point conversion in the frontend or Design IR. |
| `CC-REQ-LANG-004` | Resolution and elaboration lower through the existing Design IR and `compile` boundary; the frontend may not bypass canonical or backend validation. |
| `CC-REQ-LANG-005` | Unsupported syntax, unresolved identities, dimensional errors, and overflow produce stable human-readable and machine-readable diagnostics with source locations. |
| `CC-REQ-LANG-006` | A headless `circuitc compile` command is available through Bazel and returns meaningful exit status for success, source errors, and I/O failures. |
| `CC-REQ-LANG-007` | Semantically unordered declaration order and independent compiler processes do not affect Design IR meaning or generated artifact bytes. |
| `CC-REQ-LANG-008` | The source-authored fixture and Rust bootstrap fixture are equivalent at the Design IR boundary and remain accepted by KiCad and Ohmnivore. |

## Current vertical slice

Compile `examples/voltage_divider.circuitc` through:

```text
source -> syntax tree with spans -> resolution/elaboration
       -> canonical Design IR -> existing compile boundary
       -> KiCad PCB and SPICE artifacts
```

The source fixture becomes the primary authored example. The Rust fixture
remains a regression oracle until the frontend is proven.

## Non-goals

- Hierarchy, typed interfaces, or explicit no-connects.
- General KiCad symbol or footprint ingestion.
- KiCad schematic or project generation.
- New simulation devices or analyses.
- Direct Ohmnivore or APGAR integration.
- Backwards-compatibility infrastructure for unreleased syntax or schemas.

## Acceptance gates

- Parser, exact-quantity, elaboration, diagnostic, and CLI tests pass through
  Bazel.
- Source declaration permutations produce identical semantics and artifacts.
- Two independent CLI processes produce byte-identical KiCad and SPICE files.
- The source and Rust fixtures produce byte-identical KiCad output and
  equivalent SPICE output and name maps.
- `bazel build //...`, `bazel test //...`, strict action lockfile tests, and
  strict module-graph validation pass.
- `bazel test //:kicad10_drc_test` remains passing.
- Generated SPICE produces `VOUT` within `1e-6 V` of `5 V` in the supported
  Ohmnivore CPU solver when available.

## Completion evidence

Not yet complete. Record the implementation commit, exact gates, generated
artifact comparison, KiCad report, and Ohmnivore result here before changing
the status.
