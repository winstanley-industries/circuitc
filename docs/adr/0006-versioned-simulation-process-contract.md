# ADR-0006: Simulation uses versioned process contracts

- Status: Accepted
- Date: 2026-08-02

## Context

EPIC-0003 makes simulation a checked compiler phase. CircuitC must express
analysis and assertion intent without making a simulator's floating-point
matrix or solver IR canonical. The selected initial simulator, Ohmnivore
0.1.0, accepts SPICE files and writes analysis-dependent CSV to standard
output. Its CLI exposes neither a versioned result schema nor a capability
endpoint, and multiple analyses produce concatenated result sets without
stable analysis identifiers.

CircuitC also needs deterministic evidence that joins backend names to
canonical identities, rejects stale or malformed results, and records failed
or unevaluated assertions explicitly. Raw solver output is not sufficient for
that contract.

## Decision

CircuitC owns four strict version-1 JSON contracts:

1. a simulation request naming exactly one analysis, every assertion intent
   for that analysis, the selected backend, the canonical relative input
   paths, and the SHA-256 digest of the exact netlist bytes;
2. a SPICE identity map containing sorted, injective net and device mappings
   and the SHA-256 digest of the exact request bytes;
3. a normalized result containing canonical identities, finite canonical
   numeric strings, execution status, and the request and map digests; and
4. an assertion report containing explicit `pass`, `fail`, `unsupported`, or
   `unevaluated` outcomes and the request, map, and result digests.

The digest graph is acyclic. The request binds the netlist and names the map
path; the map binds the request; the result binds request and map; and the
report binds request, map, and result. A JSON artifact's digest covers its
canonical UTF-8 bytes, including the required final newline.
Consumers parse only that one canonical JSON byte encoding, recompute every
predecessor digest, and verify matching design, analysis path, and analysis
kind across the chain before accepting the artifact. Result signals must join
through the map, and reports must contain one outcome for every authenticated
request assertion and join pass/fail values to exact normalized samples.

Ohmnivore remains behind a process boundary. Bazel owns the exact source
revision `c2189a651d4879211019e109b2136dee836a5c5d`, builds the executable, and
passes it to the CircuitC adapter through runfiles. The initial adapter accepts
only Ohmnivore `0.1.0`, invokes `--cpu`, and advertises only the declared linear
device and analysis subset. It invokes the process once per analysis so CSV
shape and analysis identity are unambiguous. Ohmnivore's internal Rust types,
solver matrices, and floating-point state do not enter the Design IR.

Raw CSV, stderr, temporary paths, process timing, and host environment are
ephemeral. CircuitC parses them strictly, maps results only through the
identity map, rejects non-finite or incomplete values, and emits normalized
contracts. Successful exit status alone is not acceptance.

ngspice is an independent numerical authority for overlapping coverage. The
initial differential gate requires ngspice 45.2 and remains a Bazel-owned
`local`/`manual` host gate until that executable is provisioned hermetically in
CI. Its unavailability is reported, never treated as passing evidence.

## Consequences

- CircuitC can change simulator implementations without changing canonical
  connectivity or assertion intent.
- One analysis produces one independently checksummed request, map, result,
  and report chain.
- Result serialization is byte-deterministic even though the solver boundary
  uses floating point.
- Unsupported capabilities, solver failure, malformed output, stale bindings,
  and incomplete evaluation become explicit machine-readable failures.
- Updating the pinned Ohmnivore revision, accepted CLI contract, normalized
  result semantics, or ngspice authority requires a reviewed contract change.
- A future in-process Ohmnivore adapter must preserve these contracts and
  requires a separate architecture decision if it changes the authority
  boundary.
