# CircuitC language

## Status and authority

This document defines the active, unreleased M1B grammar. It is deliberately
limited to the reference voltage-divider project semantics. The grammar may evolve in
place before the first language release; there is no version declaration or
compatibility machinery yet.

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
```

A resistor value uses `ohm` or `kohm`. A DC source value uses `V`. Lengths must
convert to an exact integer number of nanometres. The frontend rejects
sub-nanometre precision, integer overflow, values outside the Design IR
coordinate envelope, dimensional mismatch, and electrical exponents outside
`[-18, 18]`. Electrical values fold insignificant decimal zeros into a unique
canonical coefficient and exponent, so forms such as `10 kohm` and
`10000 ohm` elaborate identically.

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

Once every artifact has been published, failure to remove backup staging does
not misreport publication as failed: the CLI emits `CC-CLI-IO-003`, names the
cleanup residue, and exits successfully. Failures before complete publication
still roll back and exit `3` with `CC-CLI-IO-002`.

Human diagnostics are the default. JSON diagnostics contain stable codes, the
requested filename, UTF-8 byte spans, one-based line and column, semantic path
when available, deterministic messages, and related locations for duplicates.

## Deferred language work

Reusable parameterized module definitions, general third-party library
ingestion, new simulation devices and analyses, and released-language
compatibility are deferred beyond the initial M1B catalog and instance-tree
slice.
