# ADR-0001: Rust compiler core with Bazel as the build interface

- Status: Accepted
- Date: 2026-08-01

## Context

CircuitC needs a language frontend, semantic analysis, exact domain types,
deterministic file generation, process orchestration, and integrations with
existing Rust and C++/CUDA projects. The initial language choice is Rust or
C++20. Bazel is required.

APGAR is already a C++/CUDA Bazel library and should remain so. Ohmnivore is
already a Rust library. Choosing C++ for CircuitC would simplify a future
in-process APGAR call but would make the compiler and Ohmnivore boundary more
expensive. Choosing Rust gives the orchestration and semantic layers stronger
sum types, ownership, exhaustive matching, and memory safety without requiring
APGAR to move languages.

Modern `rules_rust` supports Bzlmod, hermetic Rust toolchains, Rust 2024,
macOS/Linux, formatting, and Clippy under current Bazel releases.

## Decision

CircuitC's compiler core, canonical Design IR, CLI, and first-party backend
adapters are written in Rust 2024.

Bazel is the only top-level build, test, lint, and execution interface. The
toolchain is pinned through Bzlmod. A top-level Cargo workspace is not added.

APGAR stays C++/CUDA. Its initial CircuitC integration is a versioned serialized
process boundary. A narrow C ABI or `cxx` bridge may be added later if profiling
shows the process boundary is material and the schema has stabilized.

Ohmnivore initially integrates through SPICE and structured results. It may
later become a Bazel Rust dependency without changing CircuitC's canonical IR.

## Consequences

- The core is well suited to parser, diagnostics, and deterministic compiler
  work.
- Ohmnivore can eventually integrate without a language boundary.
- APGAR retains its current CUDA and exact-geometry implementation.
- Cross-project schemas and capability negotiation are required and cannot be
  replaced by sharing internal structs.
- Developers use Bazel even when a Rust-only change could be built with Cargo.

## References

- [`rules_rust` in the Bazel Central Registry](https://registry.bazel.build/modules/rules_rust)
- [`rules_rust` Bzlmod and toolchain setup](https://bazelbuild.github.io/rules_rust/)
