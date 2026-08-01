# CircuitC

CircuitC is a headless, code-oriented compiler for electronic systems. Its
source model is intended to cover electrical intent, parts, simulation,
physical layout, and routing while treating EDA packages as validated output
backends rather than the source of truth.

The first backend is KiCad 10. The bootstrap vertical slice is deliberately
small but end to end: a Rust-authored voltage-divider design is validated once
and lowered deterministically to both a KiCad PCB and a SPICE netlist.

## Why Rust

CircuitC's compiler and orchestration layer is Rust. Ohmnivore is already a
Rust simulation library, while APGAR remains a C++/CUDA routing library behind
a versioned Board IR boundary. Bazel is the only supported build interface, so
the repository does not carry a competing Cargo workspace.

See [the architecture](docs/architecture.md) and
[ADR-0001](docs/adr/0001-rust-bazel-compiler-core.md) for the full rationale.

## Build and test

```sh
bazel build //...
bazel test //...
bazel test --lockfile_mode=error //...
bazel mod graph --lockfile_mode=error
```

Generate the reference design:

```sh
bazel run //:voltage_divider -- /tmp/circuitc-voltage-divider
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

The reference board's present pads are connected. The M0 host gate reports zero
unconnected items and two expected warnings because the embedded bootstrap
`CircuitC` footprint is not yet installed as a vendored KiCad library. Library
ingestion and warning-free host DRC are M1 work.

## Current boundary

Implemented now:

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

- the CircuitC source language and parser;
- KiCad schematic and project emission;
- vendored symbol/footprint library ingestion;
- direct Ohmnivore execution;
- APGAR Board IR lowering and route import; or
- production ERC/DRC rule coverage.

The Rust API used by the reference design is a bootstrap frontend, not a
commitment that end-user circuit descriptions must be Rust.
