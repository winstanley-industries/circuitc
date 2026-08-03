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

Each analysis uses one deterministic artifact directory. Its
`<analysis-stem>` is the lowercase hexadecimal SHA-256 of
`circuitc-simulation-path-v1\0<design>\0<analysis-path>`, where `<design>` is
the design name and `<analysis-path>` is the analysis semantic path. The
complete per-analysis chain has exactly these generated paths:

```text
simulation/<analysis-stem>/analysis.spice
simulation/<analysis-stem>/request.json
simulation/<analysis-stem>/spice-map.json
simulation/<analysis-stem>/result.json
simulation/<analysis-stem>/report.json
```

CircuitC executes every declared analysis in semantic-path order and evaluates
every authenticated assertion instead of stopping at the first checked
failure. The checked phase succeeds only when every per-analysis result has
status `completed` and every report outcome is `pass`. A completed analysis
with no assertions therefore passes vacuously; an `unsupported`, `failed`, or
`unevaluated` result with no assertions still fails because its result is not
`completed`.

The checked compiler retains at most 64 MiB of canonical normalized-result
bytes across the complete analysis set, in addition to the lowering layer's
64 MiB aggregate input-artifact budget. It reserves deterministic failed-result
evidence for every remaining analysis before accepting another completed
result. Crossing the result budget produces `CC-SIM-CHECK-003` failed results
and unevaluated reports for the current and remaining analyses; all analyses
are still invoked under the runner's aggregate process deadline, and no
analysis disappears from the evidence set.

On checked success, CircuitC publishes all five files for every analysis in the
same failure-atomic transaction as the static design, KiCad, identity-map, and
generic SPICE artifacts. On an assertion, unsupported-capability, or simulation
runtime failure, CircuitC instead publishes the complete five-file chain for
every analysis to the deterministic sibling root `<OUTPUT_DIRECTORY>.failed`.
The requested success root remains untouched. Failure evidence begins only
after netlist, request, and map lowering has succeeded; source, semantic, or
lowering failures before that boundary cannot produce a digest-authenticated
result and report chain.

Both output roots are additive: publication replaces the paths emitted by the
current invocation but does not prune unrelated or stale paths. The emitted
relative paths, not a directory scan, identify the current artifact set. The
publication transactions provide all-or-rollback failure atomicity for their
emitted paths; they do not claim snapshot isolation for concurrent readers.
Readers consume a bundle only after the command reports successful publication.
Transaction cleanup retains the descriptor that established each staged or
backup file identity until that file is removed or restored, so an unlinked
inode cannot be recycled for a racing replacement and then pass the recorded
device/inode comparison.

Checked execution creates its private compiler work root outside the output in
the same validated namespace. For an existing output, the work root is a random
descriptor-created sibling in the output's immediate parent. For a missing
output, it is a sibling of the first missing component in the deepest existing
validated ancestor. The sibling parent is retained from the output walk and is
validated as a mutable transaction parent; it is not reconstructed through an
OS temporary path. Post-creation lexical and device/inode checks reject direct
aliases and races. This placement requires sibling-creation permission even
when an existing output directory itself is writable, and `/` is not a valid
output because it has no outside sibling.

Ohmnivore remains behind a process boundary. Bazel owns the exact source
revision `c2189a651d4879211019e109b2136dee836a5c5d`, builds the executable, and
passes it to the CircuitC adapter through runfiles. The initial adapter accepts
only Ohmnivore `0.1.0`, invokes `--cpu`, and advertises only the declared linear
device and analysis subset. It invokes the process once per analysis so CSV
shape and analysis identity are unambiguous. Ohmnivore's internal Rust types,
solver matrices, and floating-point state do not enter the Design IR.

Before executable resolution, the adapter revalidates the authenticated
netlist against the exact emitted device grammar. A resistor line is one
`R`/`r`-prefixed mapped device, two mapped nodes, and one positive value. An
independent source line is one `V`/`v`-prefixed mapped device, two mapped nodes,
uppercase `DC`, and one value; exactly one source in an AC analysis additionally
has uppercase `AC`, a non-negative magnitude, and a phase. No AC excitation is
accepted for another analysis kind. Every value must round-trip through
CircuitC's canonical exact `Quantity::spice_literal` spelling and convert to a
finite backend `f64`. Other device classes, suffix spellings, expressions,
parameters, fields, and keyword variants fail before any backend process runs.

Raw CSV, stderr, temporary paths, process timing, and host environment are
ephemeral. CircuitC parses them strictly, maps results only through the
identity map, rejects non-finite or incomplete values, and emits normalized
contracts. Successful exit status alone is not acceptance.

The `ohmnivore-cli-csv/v1` adapter grammar is recorded under
`schemas/ohmnivore_cli_csv/v1.md`. Each invocation uses a fresh private working
directory, an empty environment plus a fixed locale/timezone/thread allowlist,
null standard input, `--cpu`, and no statistics flag. The default handshake and
analysis wall limits are 2 and 30 seconds within one five-minute aggregate
runner budget. Standard output and error are drained
concurrently and bounded to 16 MiB and 64 KiB; normalized result construction
remains bounded by the 64 MiB contract envelope. CPU time, output file size,
open descriptors, and core files are limited before `exec`. Linux additionally
applies an 8 GiB hard address-space limit. Darwin does not implement a usable
finite `RLIMIT_AS`; its adapter therefore claims wall/CPU/output/file/descriptor
bounds, not a hard RSS ceiling. A hard cross-platform process-tree memory
ceiling requires a future platform containment decision rather than a silent
portability approximation.

Bazel copies the configured executable to a fixed first-party runfile and
generates its immutable provenance sidecar. Production callers supply only a
work root; they cannot substitute executable or sidecar paths. The sidecar
records the exact backend tuple and source revision plus SHA-256 of the platform
executable. The runner accepts only bounded regular runfiles, verifies that
sidecar and executable digest once per runner under a 30-second identity
deadline, and then performs
the exact `ohmnivore 0.1.0` runtime handshake. One Bazel constant supplies the
Rust contract and provenance writer revision; a Bazel test also requires the
module pin and committed resolved Git commit to match it. The committed Bazel
module lock, rather than Ohmnivore's Cargo lock, owns the resolved transitive
graph for this process binary.

The version handshake and analysis use different private directories. The
analysis netlist is atomically created with exclusive/no-follow semantics before
the backend enters that directory, and every post-creation outcome performs
checked cleanup before returning. Process-group termination contains accidental
descendants of the exact provenance-authenticated Ohmnivore binary; it is not a
general sandbox against a malicious same-user executable that deliberately
escapes its process group. Supporting interchangeable untrusted simulator
binaries would require a separate OS containment decision.

The pinned linear AC grid has a deterministic backend operation order, so the
adapter can lower an exact authored frequency to its generated backend row in
advance. Ohmnivore's transient solver is adaptive even for the initial R/Vdc
subset, so CircuitC does not reconstruct or authenticate a nominal repeated-
addition time axis. A transient request instead binds the direct backend-parser
conversion of each authored exact assertion time. The runner validates the
axis actually emitted, including exact requested-row and declared-stop
presence, and fails closed when adaptive integration does not provide them.
Distinct exact transient controls or assertion samples that alias at the
`f64` boundary are rejected during lowering.

ngspice is an independent numerical authority for overlapping coverage. The
initial differential gate requires ngspice 45.2 and remains a Bazel-owned
`local`/`manual` host gate until that executable is provisioned hermetically in
CI. Its unavailability is reported, never treated as passing evidence.

## Consequences

- CircuitC can change simulator implementations without changing canonical
  connectivity or assertion intent.
- One analysis produces one independently checksummed request, map, result,
  and report chain.
- Successful checked compilation publishes each complete five-file analysis
  chain together with the successful design bundle; checked failure retains
  complete evidence separately without modifying that bundle.
- Result serialization is byte-deterministic even though the solver boundary
  uses floating point.
- Unsupported capabilities, solver failure, malformed output, stale bindings,
  and incomplete evaluation become explicit machine-readable failures.
- Updating the pinned Ohmnivore revision, accepted CLI contract, normalized
  result semantics, or ngspice authority requires a reviewed contract change.
- A future in-process Ohmnivore adapter must preserve these contracts and
  requires a separate architecture decision if it changes the authority
  boundary.
