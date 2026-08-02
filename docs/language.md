# CircuitC language

## Status and authority

This document defines the active, unreleased M2 grammar. It is deliberately
limited to the reference circuit and simulation-closure semantics. The grammar
may evolve in place before the first language release; there is no version
declaration or compatibility machinery yet.

CircuitC source is authoritative. Parsing and elaboration produce the canonical
Design IR, and only the existing compiler boundary may lower that IR to KiCad or
SPICE. Requested source filenames and output paths are I/O data, never semantic
identity inputs. Diagnostics retain the requested filename; the deterministic
identity manifest uses the logical source name `<design>.circuitc`.

## Lexical form

- Source is UTF-8; all diagnostic spans are half-open UTF-8 byte ranges.
- Whitespace is insignificant. `//` starts a line comment even when adjacent
  to a preceding identity; the sequence is therefore not part of an identity.
- Keywords and units are case-sensitive.
- Bare identities contain ASCII letters, digits, `_`, `+`, `-`, `.`, or `/`.
- Design names, which become artifact stems, are narrower: an ASCII letter or
  `_` followed by ASCII letters, digits, `_`, or `-`.
- Decimal literals contain an optional `-`, one or more digits, and an optional
  fractional part. They are parsed as integers plus a decimal scale, never as
  floating point.
- Footprint library identifiers are quoted strings. Only `\"` and `\\` escapes
  are supported.

## Grammar

The notation below uses `*` for repetition and `?` for optional forms.

```text
source      := "design" name "{" declaration* "}"
declaration := ("net" | "ground") name ";"
             | module
             | component
             | analysis
             | assertion
             | board

module      := "module" path "{" port* "}"
port        := "port" direction name electrical_type connection_state ";"
direction   := "input" | "output" | "inout"
connection_state := "connect" net | "no_connect"

component   := ("resistor" | "dc_source") path reference
               "{" component_item* "}"
component_item := part | symbol | model | schematic_position
                | value | terminals | connection | no_connect | footprint
part        := "part" string
               ("manufacturer" string "number" string | "virtual") ";"
symbol      := "symbol" string "{" symbol_pin* "}"
symbol_pin  := "bind" pin symbol_pin_number electrical_type ";"
model       := "model" string ";"
schematic_position := "schematic" "at" point "rotation" integer "deg" ";"
value       := ("resistance" | "voltage") quantity ";"
terminals   := "terminals" pin pin ";"
connection  := "connect" pin net ";"
no_connect  := "no_connect" pin ";"
footprint   := "footprint" string "{" binding* "}"
binding     := "bind" pin pad_number ";"

board       := "board" "{" board_item* "}"
board_item  := "rectangle" "at" point "size" point ";"
             | "place" reference "at" point "rotation" integer "deg"
               "layer" layer ";"
             | "route" path "net" net "from" point "to" point
               "width" length "layer" layer ";"

point       := "(" length "," length ")"
quantity    := decimal unit
length      := decimal ("nm" | "um" | "mm")
layer       := "front" | "back"
electrical_type := "input" | "output" | "bidirectional" | "passive"
                 | "power_input" | "power_output"
                 | "open_collector" | "open_emitter"

analysis    := "analysis" "dc_operating_point" path ";"
             | "analysis" "ac_linear_sweep" path
               "source" path "points" integer
               "start_frequency" frequency "stop_frequency" frequency
               "magnitude" voltage "phase" angle ";"
             | "analysis" "transient" path
               "step" time "stop" time "start" time
               "uic" boolean ";"

assertion   := "assert" "net_voltage" path
               "analysis" path "net" net "sample" sample
               "expected" voltage
               "absolute_tolerance" voltage
               "relative_tolerance" ratio ";"
sample      := "scalar" | "frequency" frequency | "time" time
frequency   := decimal ("Hz" | "kHz")
time        := decimal ("s" | "ms" | "us")
voltage     := decimal "V"
angle       := decimal "deg"
ratio       := decimal "ratio"
boolean     := "true" | "false"
```

A resistor value uses `ohm` or `kohm`. A DC source value uses `V`. Lengths must
convert to an exact integer number of nanometres. The frontend rejects
sub-nanometre precision, integer overflow, values outside the Design IR
coordinate envelope, dimensional mismatch, and electrical exponents outside
`[-18, 18]`. Electrical values fold insignificant decimal zeros into a unique
canonical coefficient and exponent, so forms such as `10 kohm` and
`10000 ohm` elaborate identically.
Orthogonal rotation is counterclockwise as rendered; in KiCad's Y-down frame,
an offset `(x, y)` at 90 degrees maps to `(y, -x)`.

Simulation intent is always explicit. A legacy design with no `analysis`
declaration has no implicit operating point or other analysis. Analysis and
assertion paths are stable semantic identities and declaration order does not
affect their canonical Design IR order. `points` is an exact integer in
`2..=10,000`; frequency, time, magnitude, and tolerance values remain exact decimal
quantities through Design IR elaboration. A DC operating-point assertion uses
`sample scalar`, an AC assertion uses `sample frequency`, and a transient
assertion uses `sample time`. AC samples must lie exactly on the declared
endpoint-inclusive linear grid. Transient samples must be zero-anchored integer
multiples of `step` within the output interval, or the forced exact `stop`
endpoint. The referenced analysis, source component, and net are resolved by
Design validation, which also rejects invalid dimensions,
nonpositive frequencies, transient steps, or transient stops; negative
transient starts, AC magnitudes, or tolerances; incompatible sample kinds; and
ranges ordered backwards. Negative expected voltages and AC phase values are
valid. A design may declare at most 256 analyses and 10,000 assertions. No
analysis may exceed 10,000 nominal samples, and the aggregate declared grid
across all analyses may not exceed 10,000. A transient's nominal grid is
budgeted from time zero as `ceil(stop / step) + 1`; `start` filters output but
does not reduce that declaration bound. Ohmnivore's transient solver is
adaptive, so actual accepted and rejected solver steps are instead bounded by
the checked process adapter and are never inferred from this nominal count.

Each declared analysis deterministically lowers to its own SPICE netlist,
versioned request, and standalone reversible identity map. Component references
remain canonical CircuitC identities and need not use a SPICE device prefix;
the backend preserves only safe model-compatible names and otherwise owns an
injective derived name. Exact quantities cross to the pinned backend's `f64`
parser only in this lowering step. CircuitC rejects collapsed or non-increasing
AC axes, distinct transient controls that collapse to one backend value, and
distinct exact transient assertion samples that would alias. The request
authenticates authored transient samples without predicting the adaptive
solver's output rows; checked execution must reject a missing assertion row or
declared stop. Lowering also applies a conservative 64 MiB aggregate budget to
all retained per-analysis input artifacts before constructing them. These
failures use `CC-SIM-LOWER-*` diagnostics. Until checked process execution is
added, a successfully lowered analysis still fails closed with
`CC-SIM-PHASE-001` and no static artifact bundle is published.

Each component has exactly one part, symbol, schematic position, and
kind-appropriate value, plus at most one footprint. `model` and `terminals` are
an optional pair: omitting both authors a physical-only component, while
supplying exactly one is an error. A physical part names a manufacturer and
manufacturer part number; a non-physical part is explicitly `virtual`. The
logical device, manufacturer, manufacturer part number, symbol, and optional
footprint must resolve as one coherent vendored catalog entry. Footprint pad
geometry is ingested from that catalog rather than copied into source.
For the initial KiCad catalog, each bound symbol pin number must equal its
corresponding footprint pad number. A cross-mapped but otherwise valid Design
IR is rejected by the KiCad backend until the catalog grows an explicit
pin-equivalence contract.

Each component path has at least one dot and its parent path names a declared
module. Dotted module paths require their parent module. Module ports carry
direction, electrical pin type, and explicit connection state. Every symbol
pin has exactly one logical binding and every bound logical pin has exactly one
`connect` or `no_connect` declaration. Simulation terminals must be connected.
An explicit physical no-connect remains absent from canonical nets; KiCad
lowering emits a deterministic backend-only `unconnected-(<ref>-Pad<pad>)` net
for the corresponding pad so the host parity checker sees the intended open.

Declarations are resolved by identity rather than order. Net, module, port,
component, symbol-pin binding, footprint binding, placement, and route
collections are canonicalized before the Design IR is exposed. Every simulated
design has exactly one `ground` declaration.

## CLI and exit status

```sh
bazel run //cmd/circuitc -- compile INPUT \
  --output-dir OUTPUT_DIRECTORY \
  [--diagnostic-format=human|json] [--]
```

The command accepts exactly one input. It transactionally writes a complete
KiCad project bundle, `<design>.kicad-map.json`, and `<design>.spice` only after
parsing, elaboration, Design validation, KiCad identity validation, project
lowering, and SPICE lowering all succeed.

Every generated artifact name must be a safe relative path. The output root
and each generated parent are pinned with no-follow directory handles; staging,
backup, publication, rollback, and cleanup are descriptor-relative and use
no-replace renames. A symlink or concurrent directory swap therefore fails as
an I/O error instead of escaping the requested directory. Platforms without
the required anchored filesystem primitives fail closed. The current anchored
implementation supports Linux and macOS on x86_64 and aarch64. It deliberately
rejects every symlinked output-path ancestor; on macOS, callers that need the
system temporary directory must therefore use its canonical `/private/tmp`
path rather than the `/tmp` symlink.

- `0`: success;
- `1`: source, semantic, or backend-validation failure;
- `2`: invalid invocation or unsupported option; and
- `3`: input or output I/O failure.

Once every artifact has been published, CircuitC synchronizes each distinct
artifact parent and each parent in which it created a directory, after removing
backup staging. A post-publication directory-sync or backup-cleanup failure does
not misreport publication as incomplete: the CLI emits `CC-CLI-IO-003`, names
the durability or cleanup failure, and exits successfully. Failures before
complete publication still roll back and exit `3` with `CC-CLI-IO-002`. A
broken output pipe after publication is also treated as success. Other
success-reporting stream failures emit `CC-CLI-IO-004`, explicitly state that
outputs were already published, and exit `3`.

Human diagnostics are the default. JSON diagnostics contain stable codes, the
requested filename, UTF-8 byte spans, one-based line and column, semantic path
when available, deterministic messages, and related locations for duplicates.

## Deferred language work

Reusable parameterized module definitions, general third-party library
ingestion, additional simulation devices and analysis forms, and
released-language compatibility are deferred beyond the current M2 slice.
