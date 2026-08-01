# EPIC-0001: CircuitC language frontend

- Status: Complete
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

Completed on 2026-08-01 in the intentionally uncommitted working tree based on
`f976aab2e85da37bf7e8b5360b5d39d6273c65ae` (`main`). The implementation was
not committed or pushed at the request of the operator. The SHA-256 of the
sorted per-file SHA-256 manifest for the implementation payload is
`8488de30155e0f80fab81d768675797b38f3ce7a78e8ce9d4917685ded20e10a`;
the two self-recording epic status/evidence files are excluded from that
manifest.

| Requirement | Implementation ownership | Tests and completion evidence |
| --- | --- | --- |
| `CC-REQ-LANG-001` | `src/frontend/{lexer,parser,syntax}.rs`, `docs/language.md`, and `examples/voltage_divider.circuitc` | Lexer/parser unit tests cover the documented minimal grammar, unsupported declarations, recovery, comments, and digit-leading canonical identities; `//:circuitc_test` passes. |
| `CC-REQ-LANG-002` | `SourceFile`, spanned syntax nodes, diagnostics, and the typed semantic/structural provenance side table under `src/frontend/` | UTF-8 byte-span, source-identity, route mapping, and adversarial structural and synthesized-path collision regressions pass in `//:circuitc_test`. No parser node is present in Design IR. |
| `CC-REQ-LANG-003` | `src/frontend/quantity.rs`, `src/quantity.rs`, and the active Design IR schema | Exact decimal, unit, sub-nanometre, overflow, canonical equivalence, exponent-boundary, long-insignificant-zero, and full-turn rotation tests pass without frontend floating point. |
| `CC-REQ-LANG-004` | `src/frontend/elaborate.rs`, `src/frontend/mod.rs`, and the existing `compile()` boundary | `compile_source` lowers syntax through canonical `Design`, `Design::validate`, KiCad identity validation, and SPICE name lowering; the equivalence helper reports equal Design IR, artifacts, and name maps. |
| `CC-REQ-LANG-005` | `src/frontend/diagnostic.rs` plus lexer, parser, resolution, elaboration, and mapped IR/backend diagnostics | Required diagnostic categories, deterministic ordering, related duplicate locations, human rendering, stable JSON golden output, filename-only path variance, and source-span mapping pass in `//:circuitc_test` and `//:frontend_cli_test`. |
| `CC-REQ-LANG-006` | `//cmd/circuitc` | Bazel-built `compile`, output directory, diagnostic format, `--` terminator, status codes 0/1/2/3, unsupported-option handling, and transactional two-artifact publication are covered by `//cmd/circuitc:circuitc_cli_test` and `//:frontend_cli_test`. |
| `CC-REQ-LANG-007` | Design canonicalization, ordered elaboration collections, deterministic diagnostics, and transactional CLI output | Declaration/item permutations preserve Design and artifacts; two independent CLI processes produced byte-identical PCB and SPICE files; absolute source/output paths do not enter identities or bytes. |
| `CC-REQ-LANG-008` | Source example, retained `demo::voltage_divider()` oracle, `tools/frontend/equivalence.rs`, and the source-driven KiCad gate | Source and Rust paths are equal at Design IR, KiCad, SPICE, and reversible-name-map boundaries; KiCad 10.0.5 accepts the PCB; Ohmnivore CPU returns `V(VOUT) = 4.999999975 V`. |

Exact successful gates from the final working tree:

```text
bazel build //...
bazel test //...
bazel test --lockfile_mode=error //...
bazel mod graph --lockfile_mode=error
bazel build //:clippy
bazel test //:kicad10_drc_test --test_output=all
```

The standard and strict-lockfile suites each reported all five tests passing.
The host authority gate used KiCad 10.0.5, reported zero unconnected items, and
contained only the two expected `lib_footprint_issues` warnings because the
host configuration does not install the source-authored `CircuitC` footprint
library. No required gate was unavailable.

Two independent invocations of `bazel run //cmd/circuitc -- compile
examples/voltage_divider.circuitc --output-dir ...` compared equal with `cmp`.
The source and Rust bootstrap files also compared equal. Their SHA-256 values
were:

```text
e859a70c54365a973e3958c9030b07734f01db1bbc39fa298809b13acf153fb3  voltage_divider.kicad_pcb
4638ce4cf1b6f8d73bba053b2919ac653e340f00291acfe5af2afdd006401444  voltage_divider.spice
```

The current Ohmnivore 0.1.0 CPU executable from repository commit
`c2189a651d4879211019e109b2136dee836a5c5d` (executable SHA-256
`307f48f7d3d003dff441bb3369b611e544278d1a96c06332436dbd0b68407487`)
produced `V(VIN) = 10 V`, `V(VOUT) = 4.999999975 V`, and
`I(V1) = -0.0005000000125 A`. The VOUT absolute error is `2.5e-8 V`, within
the required `1e-6 V` tolerance.

Three independent adversarial passes reviewed parser recovery/spans and
diagnostics, exact quantity/identity/determinism behavior, and CLI/Bazel/backend
authority. Confirmed findings involving adjacent comments, digit-leading
identities, braced recovery, typed provenance collisions, decimal
normalization, failure-atomic artifact publication, and leading-dash paths were
fixed and regression-tested. The refreshed reviews reported no remaining P1 or
P2 findings.
