# CircuitC

CircuitC is a headless, code-oriented compiler for electronic systems. Its
source model is intended to cover electrical intent, parts, simulation,
physical layout, and routing while treating EDA packages as validated output
backends rather than the source of truth.

The first backend is KiCad 10. The current vertical slice is deliberately small
but end to end: a file-authored voltage-divider design is parsed, elaborated,
validated once, and lowered deterministically to an isolated KiCad schematic,
PCB, project and vendored-library bundle, plus a SPICE netlist. The original
Rust fixture remains an equivalence oracle.

## Why Rust

CircuitC's compiler and orchestration layer is Rust. Ohmnivore is already a
Rust simulation library, while APGAR remains a C++/CUDA routing library behind
a versioned Board IR boundary. Bazel is the only supported build interface, so
the repository does not carry a competing Cargo workspace.

See [the architecture](docs/architecture.md), [the epic
roadmap](docs/epics/README.md), and
[ADR-0001](docs/adr/0001-rust-bazel-compiler-core.md) for the rationale and
planned outcomes.

## Build and test

```sh
bazel lint //...
bazel build //...
bazel test //...
bazel test --lockfile_mode=error //...
bazel mod graph --lockfile_mode=error
```

`bazel lint` is provided by the repository's Bazelisk wrapper and runs the
Bazel-pinned rustfmt, Clippy, Buildifier, Ruff, and ShellCheck gates. Use
`bazel lint --fix` to apply supported formatters, or select one check while
iterating, for example `bazel lint --only clippy` or
`bazel lint --only buildifier`.

GitHub Actions runs these gates on Linux for pull requests, merge queues, and
pushes to `main`. After the Linux gate passes on a same-repository pull request,
Claude Code performs an architecture-aware review and requires one fresh formal
verdict for that exact PR head. The review job uses the
`CLAUDE_CODE_OAUTH_TOKEN` Actions secret; fork and Dependabot pull requests do
not consume the token.

Compile the reference source design:

```sh
bazel run //cmd/circuitc -- compile \
  examples/voltage_divider.circuitc \
  --output-dir /private/tmp/circuitc-voltage-divider
```

This writes:

- `/private/tmp/circuitc-voltage-divider/voltage_divider.kicad_sch`
- `/private/tmp/circuitc-voltage-divider/voltage_divider.kicad_pcb`
- `/private/tmp/circuitc-voltage-divider/voltage_divider.kicad_pro`
- `/private/tmp/circuitc-voltage-divider/CircuitC.kicad_sym`
- `/private/tmp/circuitc-voltage-divider/CircuitC.pretty/R_0603_1608Metric.kicad_mod`
- `/private/tmp/circuitc-voltage-divider/sym-lib-table`
- `/private/tmp/circuitc-voltage-divider/fp-lib-table`
- `/private/tmp/circuitc-voltage-divider/voltage_divider.kicad-map.json`
- `/private/tmp/circuitc-voltage-divider/voltage_divider.spice`

When KiCad 10 is installed, run the Bazel-owned host validation gate with:

```sh
bazel test //:kicad10_drc_test
```

The gate discovers `kicad-cli` on `PATH` and at the standard macOS KiCad 10
application path. Set `CIRCUITC_KICAD_CLI` through Bazel's test environment for
another installation location. It generates the full bundle twice, byte-checks
it, parses the vendored symbol and footprint in an isolated KiCad configuration,
and accepts only normalized structured ERC/DRC reports with zero unexpected,
unconnected, or schematic-parity findings.

## Current boundary

Implemented now:

- the minimal unreleased CircuitC language, byte-spanned parser, and exact
  elaboration described in [the language reference](docs/language.md);
- deterministic human and JSON source diagnostics plus a headless Bazel CLI;
- exact integer-nanometre board coordinates;
- exact decimal electrical quantities;
- a versioned, validated semantic Design IR with module-instance hierarchy,
  typed ports and pins, explicit connection state, part identity, and
  symbol-pin/footprint-pad/model bindings;
- deterministic RFC 9562 UUIDv8 identifiers derived from semantic paths;
- code-authored schematic and PCB placement plus route segments;
- deterministic KiCad 10 schematic, PCB, project, local library tables, and
  source identity-map output;
- a small vendored KiCad symbol/footprint catalog resolved during elaboration;
- isolated KiCad 10 symbol/footprint parsing, ERC, DRC, connectivity, and
  schematic-parity validation; and
- SPICE output plus a reversible backend-name map suitable for the supported
  Ohmnivore subset.

Not implemented yet:

- direct Ohmnivore execution;
- APGAR Board IR lowering and route import; or
- broad component-library, multi-sheet, and production ERC/DRC rule coverage.

The Rust-authored reference remains a regression oracle for frontend and
backend equivalence; `.circuitc` is the primary authored form.
