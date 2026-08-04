# CircuitC Architecture

## 1. Purpose

CircuitC is a headless compiler for electronic systems. One source model should
be able to express and check:

- hierarchical electrical intent and connectivity;
- exact part identity, pins, symbols, footprints, and models;
- dimensional values, tolerances, assertions, and selection constraints;
- simulation analyses and acceptance criteria;
- board stack-up, outline, placement, geometry, and route constraints; and
- generated schematic, layout, routing, reports, and manufacturing artifacts.

KiCad 10 is the first EDA output. CircuitC is not a KiCad plugin and does not
require a running editor. KiCad remains a host validator and a useful viewer of
generated artifacts, but it is not the source of truth.

## 2. Architectural thesis

CircuitC is a compiler and orchestrator, not a new monolithic EDA kernel.
Electrical intent, physical geometry, and simulation are related views with
different correctness domains. They share stable identity and connectivity,
but they do not share one catch-all object model.

```text
CircuitC source or API
        |
        v
parse -> resolve -> elaborate -> solve/check constraints
        |
        v
Canonical Design IR (identity, hierarchy, parts, pins, nets, intent)
        |
        +--> KiCad view ------> .kicad_sch / .kicad_pcb -> KiCad ERC/DRC
        |
        +--> Simulation view -> SPICE / Ohmnivore ------> assertions/results
        |
        +--> Physical view ---> APGAR Board IR ---------> validated routes
        |
        +--> Product view ----> BOM / assembly / manufacturing outputs
```

The arrows are explicit lowering passes. A backend may reject semantics it
cannot represent; it may not silently discard or reinterpret them.

## 3. Authority and ownership

1. CircuitC source is the human-authored authority.
2. The canonical Design IR is the compiler authority for one elaborated build.
3. Specialized backend IRs are disposable, versioned compiled artifacts.
4. KiCad files, SPICE files, and CircuitC-normalized reports are deterministic
   build outputs. Raw host-tool reports are ephemeral validation evidence and
   may contain timestamps, absolute paths, or other host metadata.
5. KiCad ERC/DRC is authoritative for whether the KiCad output is accepted by
   the supported host version.
6. Ohmnivore or another selected simulator is authoritative for its numerical
   result, while CircuitC owns model mapping and assertion semantics.
7. APGAR owns route search and exact routing validation within its declared
   rule subset. KiCad DRC still gates committed KiCad routes.
8. CircuitC source and canonical Design IR own product policy, approved
   substitutions, variants, population state, configuration, and requested
   manufacturability assertions.
9. A strict checksum-pinned catalog snapshot owns only its point-in-time remote
   observations and authored validity interval. The offline resolver
   authenticates its exact canonical bytes, joins exact part identities, and
   may prove or fail authored function, value, package, lifecycle, and sourcing
   constraints. It may not add product intent, select an unapproved substitute,
   consult the host clock, or become a build-time network dependency.

Edits made only to generated KiCad files are not round-tripped. Code-authored
placement and routing belong in CircuitC source. A future importer may help
transcribe an existing project, but import is an adapter, not a second source
of truth.

## 4. Canonical Design IR

The Design IR contains concepts common to all useful views:

- source-stable identity, with source spans retained in a frontend provenance
  side table rather than embedded in canonical IR values;
- module and instance hierarchy;
- typed interfaces, ports, pins, and nets;
- exact dimensional quantities, ranges, and tolerances;
- explicit part, symbol, footprint, and simulation-model bindings;
- assertions with proof status rather than unchecked annotations; and
- physical and simulation intent attached through typed extensions;
- product variants with explicit population and configuration state; and
- pinned catalog-evidence identity plus capability-declared
  manufacturability intent.

It deliberately does not embed:

- KiCad s-expression nodes or editor object pointers;
- APGAR C++ object layouts, compiled fields, or GPU handles;
- Ohmnivore MNA matrices or solver descriptors; or
- floating-point PCB coordinates.

Version 1 of the unreleased bootstrap contract is documented in
`schemas/design_ir/v1.md`. Until the first schema release, the Design IR and
pre-language Rust construction API evolve in place without version bumps,
backwards-compatibility guarantees, or migrations. Semantic changes are still
recorded in the schema and ADRs.

## 5. Exactness and determinism

- Board coordinates use signed integer nanometres. KiCad's documented
  six-decimal-place millimetre resolution is therefore represented exactly.
- APGAR lowering multiplies nanometres by two for its current 2,000,000
  database-units-per-millimetre contract, with checked arithmetic.
- Electrical quantities are signed decimal coefficients plus a base-ten
  exponent and physical dimension. A simulator converts them to floating
  point only at its adapter boundary.
- Entity identity is stable across identical builds. The bootstrap derives
  RFC 9562 UUIDv8 identifiers from the design namespace and typed,
  length-delimited semantic identity fields. The source language will also
  support explicit identities for rename-stable objects. Backends check global
  emitted-identity uniqueness before writing artifacts.
- Output order is canonical. Wall-clock time, random UUIDs, absolute paths,
  network lookups, and hash-map iteration order may not affect artifacts.
- Catalog evaluation dates are authored canonical values. Freshness decisions
  consume authenticated evidence and explicit policy; they never consult the
  build host's current date or a live remote service.

## 6. Component integrations

### 6.1 KiCad

CircuitC writes documented s-expression files and identifies itself as the
generator. The M1B slice emits an isolated KiCad 10 schematic, PCB, project,
local library tables, and the vendored symbol and footprint resources needed by
the design. The compiler derives an ordered library-file set from the symbols
and footprints actually selected by the design, so catalog growth remains
additive and cannot silently omit a new footprint asset. Each file carries an
explicit library kind, table nickname, and table-relative path from the catalog
to the table emitter; file-name parsing never decides whether an asset appears
in a generated table. Library bindings, library files, and footprint drawing
geometry are resolved before emission.
A deterministic identity manifest maps emitted KiCad UUIDs back to CircuitC
semantic paths and source spans. KiCad objects and library display names remain
backend artifacts, not canonical compiler intent.

Vendored footprint silkscreen and courtyard drawings are backend catalog data,
not canonical physical intent. KiCad lowering copies them into each board
footprint with design-derived identities so courtyard-dependent host DRC is an
active acceptance check rather than a vacuous policy entry.

The bootstrap KiCad catalog supports only parts whose symbol pin numbers equal
their corresponding footprint pad numbers. The canonical IR keeps the two
bindings explicit and independent; the KiCad backend rejects cross-mapped
parts instead of misrepresenting their connectivity.

Canonical schematic anchors are unique. The KiCad backend additionally derives
every rotated symbol-pin connection point before emission and rejects a shared
point unless its canonical connection states are identical and connected.
Coincident no-connect pins are rejected rather than merged, preventing distinct
intent from being collapsed by shared anchors.

Every backend integration test has three levels:

1. CircuitC structural and golden tests;
2. repeat-build byte comparison; and
3. supported-host parsing plus structured `kicad-cli` ERC and DRC output,
   including connectivity and schematic-to-PCB parity.

Host validation is entered through Bazel even though the installed KiCad host
is necessarily platform-local. CircuitC parses the raw report, enforces an
explicit finding policy, and emits a normalized deterministic summary. A
successful `kicad-cli` exit status alone is not acceptance evidence. Tests use
an isolated KiCad configuration and reject unexpected ERC, DRC, unconnected,
or parity findings after joining UUIDs through the identity manifest.
KiCad 10 directly parses the schematic, PCB, symbol, and footprint artifacts
used by this gate. Its CLI has no direct `.kicad_pro` parser, so CircuitC also
parses that JSON and enforces the exact deterministic project subset it emits,
including the artifact filename contract.

The KiCad IPC API is not the primary headless boundary for versions 9 and 10
because it requires a running GUI. It may later support interactive preview or
transactional host validation without entering the compiler core.

### 6.2 Ohmnivore

Ohmnivore currently exposes a Rust SPICE parser, circuit IR, compiler, and
analysis functions. CircuitC first integrates through deterministic SPICE plus
result files. This keeps the contract differential-testable against ngspice
and avoids making Ohmnivore's solver-oriented IR canonical.

Once the model and result schemas settle, CircuitC may call Ohmnivore as a
Bazel Rust dependency. Model coverage and numerical tolerances remain explicit
capabilities; unsupported devices are compile errors, not approximate
substitutions.

CircuitC net and component identities are not constrained to SPICE's token or
case rules. The SPICE lowering owns a deterministic, injective name map,
reserves simulator ground aliases, and exposes the reversible mapping needed
to associate results with canonical Design IR identities.

### 6.3 APGAR

APGAR remains a C++/CUDA Bazel library with a CAD-neutral, exact Board IR.
CircuitC lowers placed physical connectivity and rules into a versioned APGAR
request, then imports immutable route candidates or selected routes.

The initial boundary is a checksummed serialized request/result and a
process-level integration test. Bazel pins the exact APGAR source revision,
builds the CPU adapter from APGAR's public Board IR, geometry compiler, CPU
A-star, candidate-construction, and exact-admission APIs, and binds the adapter
executable digest into every result. An in-process C ABI or `cxx` bridge is
only worth adding after the schema stabilizes. APGAR's exact validation and
KiCad DRC both gate a route; neither is bypassed for convenience. A separate
canonical acceptance manifest recomputes the exact request, result, projection,
emitted-board, and normalized KiCad ERC/DRC digest joins after the supported
host has parsed the generated project. Checked compilation produces
provisional routed artifacts; only this post-host manifest denotes acceptance.

## 7. Parts and libraries

Production builds are offline and hermetic:

- symbols, footprints, 3D models, and simulation models are vendored or fetched
  through checksum-pinned Bazel repositories;
- a part binds logical function, manufacturer, manufacturer part number,
  package, lifecycle requirement, sourcing constraints, and approved
  substitutions as independent fields;
- pin-to-pad, pin-to-symbol, and pin-to-model mappings are explicit and
  validated; and
- remote catalog search may assist authoring, but a build consumes only an
  explicitly named checksum-pinned snapshot evaluated on an authored date.

The v1 product-catalog snapshot is strict compact canonical JSON with an exact
byte digest, observation and validity dates, raw-source traceability, exact
typed values, lifecycle observations, and regional quantity/lead-time
observations. Resolution is offline and all-or-nothing: every primary part and
approved alternate must resolve exactly and satisfy the source-authored
function, value, package, lifecycle, region, quantity, and lead-time policy.
The snapshot URI and raw-source digest are traceability fields, not permission
to fetch during a build and not independent proof of upstream truth when the
raw bytes are absent.

Virtual parts retain logical function and omit every physical product field.
Every design containing a physical component carries one catalog-evidence
reference and at least one explicit product variant. Variants assign exactly
one fitted, not-fitted, or approved-alternate state to every physical
component; no generated BOM or board edit may redefine that state.

The initial manufacturability intent is limited to KiCad major version 10 and
stable assertions for clean ERC, clean DRC, clean unconnected and
schematic-parity results, and a complete fabrication inventory. This is
canonical requested intent only. Manufacturing export, normalized analysis
evidence, and release-manifest closure remain separate compiled boundaries and
require later accepted decisions before CircuitC can claim a release.
The authority and initial field-level contract are recorded in
[ADR-0008](adr/0008-product-intent-and-pinned-catalog-evidence.md).
The strict snapshot and offline resolution contract are recorded in
[ADR-0009](adr/0009-strict-offline-product-catalog-snapshot.md) and
[`schemas/product_catalog_snapshot/v1.md`](../schemas/product_catalog_snapshot/v1.md).

This directly avoids hosted-backend availability becoming a build dependency.

## 8. Compiler diagnostics

Diagnostics carry a stable code, semantic path, message, and eventually a
source span plus related locations. Unsupported capability is a first-class
diagnostic category. Backend tools are invoked with structured output where
available, and their findings are mapped back to CircuitC identities.

## 9. Milestones

Durable requirements, dependencies, vertical outcomes, and completion evidence
for these milestones are tracked in the [epic index](epics/README.md). The
architecture remains authoritative for system boundaries; epics may not
silently redefine them.

### M0: executable architecture spine

- Rust/Bazel compiler library;
- validated Design IR v1 subset;
- exact quantities and coordinates;
- deterministic KiCad 10 PCB and SPICE emitters;
- code-authored voltage-divider fixture; and
- Bazel format, lint, build, and unit-test gates.

### M0.1: compiler-boundary closure

- total Design IR validation over public bootstrap values;
- explicit route identity and logical-pin-to-pad bindings;
- injective KiCad UUID and SPICE name lowering;
- Bazel-owned KiCad 10 DRC policy with normalized evidence; and
- complete strict Bazel module-lock validation.

### M1A: file-authored design through existing backends

- a minimal declarative CircuitC language with source spans;
- stable machine-readable parser and elaboration diagnostics;
- a headless compile CLI;
- lowering of the voltage-divider source fixture through the existing Design
  IR, KiCad PCB, and SPICE backends; and
- source-order and process-level deterministic golden tests.

### M1B: useful KiCad project compiler

- hierarchy, typed interfaces, explicit no-connects, and electrical pin types;
- vendored KiCad symbol and footprint ingestion;
- deterministic `.kicad_sch`, `.kicad_pcb`, and project emission;
- placement and route syntax; and
- clean-checkout KiCad ERC/DRC integration tests.

### M2: simulation as a checked compiler phase

- simulation model binding and capability checking;
- DC, AC, and transient analysis declarations;
- Ohmnivore execution through Bazel;
- ngspice differential fixtures for overlapping device coverage; and
- assertions that fail the build on numerical or model-coverage violations.

### M3: routing integration

- physical-design lowering to APGAR's exact Board IR;
- versioned request, route, provenance, and replay schemas;
- selected-route import into KiCad output;
- APGAR exact validation followed by KiCad DRC with one authenticated
  acceptance manifest; and
- deterministic CPU reference fixtures before GPU performance work.

### M4: product and manufacturing closure

- independently authored logical function, part identity, package, lifecycle
  requirement, sourcing constraints, and approved substitutions;
- checksum-pinned catalog evidence whose authored evaluation date makes
  freshness reproducible without wall-clock or network access;
- variants, BOM, placement, fabrication, and assembly outputs;
- an initial capability-declared KiCad 10 manufacturability analysis; and
- reproducible release manifests tying every artifact to source and toolchain
  identities.

## 10. Immediate non-goals

- a CircuitC GUI;
- a general-purpose constraint solver before the language and IR are proven;
- silently preserving arbitrary manual edits in generated KiCad files;
- rewriting APGAR or Ohmnivore inside this repository; and
- claiming production schematic, DRC, routing, or simulator coverage from the
  M0 fixture.
