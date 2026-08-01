# CircuitC

CircuitC is a headless, code-oriented compiler for electronic systems. Its
source model is intended to cover electrical intent, parts, simulation,
physical layout, and routing while treating EDA packages as validated output
backends rather than the source of truth.

The first backend is KiCad 10. The current vertical slice is deliberately small
but end to end: a file-authored voltage-divider design is parsed, elaborated,
validated once, and lowered deterministically to both a KiCad PCB and a SPICE
netlist. The original Rust fixture remains an equivalence oracle.

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
  --output-dir /tmp/circuitc-voltage-divider
```

This writes:

- `/tmp/circuitc-voltage-divider/voltage_divider.kicad_pcb`
- `/tmp/circuitc-voltage-divider/voltage_divider.spice`

When KiCad 10 is installed, run the Bazel-owned host validation gate with:

```sh
bazel test //:kicad10_drc_test
```

The gate discovers `kicad-cli` on `PATH` and at the standard macOS KiCad 10
application path. Set `CIRCUITC_KICAD_CLI` through Bazel's test environment for
another installation location. It parses the raw JSON rather than trusting the
host exit code and compares two normalized reports for determinism.

The reference board's present pads are connected. The host gate reports zero
unconnected items and two expected warnings because the embedded bootstrap
`CircuitC` footprint is not yet installed as a vendored KiCad library. Library
ingestion and warning-free host DRC are M1B work.

## Current boundary

Implemented now:

- the minimal unreleased CircuitC language, byte-spanned parser, and exact
  elaboration described in [the language reference](docs/language.md);
- deterministic human and JSON source diagnostics plus a headless Bazel CLI;
- exact integer-nanometre board coordinates;
- exact decimal electrical quantities;
- a versioned, validated semantic Design IR with explicit route identity and
  logical-pin-to-pad bindings;
- deterministic RFC 9562 UUIDv8 identifiers derived from semantic paths;
- code-authored placement and route segments;
- KiCad 10 PCB output; and
- SPICE output plus a reversible backend-name map suitable for the supported
  Ohmnivore subset.

Not implemented yet:

- KiCad schematic and project emission;
- vendored symbol/footprint library ingestion;
- direct Ohmnivore execution;
- APGAR Board IR lowering and route import; or
- production ERC/DRC rule coverage.

The Rust-authored reference remains a regression oracle for frontend and
backend equivalence; `.circuitc` is the primary authored form.
