# CircuitC language

## Status and authority

This document defines the active, unreleased M1A grammar. It is deliberately
limited to the reference voltage-divider semantics. The grammar may evolve in
place before the first language release; there is no version declaration or
compatibility machinery yet.

CircuitC source is authoritative. Parsing and elaboration produce the canonical
Design IR, and only the existing compiler boundary may lower that IR to KiCad or
SPICE. Source filenames and output paths are provenance and I/O data, never
semantic identity inputs.

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
             | component
             | board

component   := ("resistor" | "dc_source") path reference
               "{" component_item* "}"
component_item := value | terminals | connection | footprint
value       := ("resistance" | "voltage") quantity ";"
terminals   := "terminals" pin pin ";"
connection  := "connect" pin net ";"
footprint   := "footprint" string "{" (pad | binding)* "}"
pad         := "pad" pad_number "at" point "size" point
               "shape" ("rect" | "roundrect") ";"
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
```

A resistor value uses `ohm` or `kohm`. A DC source value uses `V`. Lengths must
convert to an exact integer number of nanometres. The frontend rejects
sub-nanometre precision, integer overflow, values outside the Design IR
coordinate envelope, dimensional mismatch, and electrical exponents outside
`[-18, 18]`. Electrical values fold insignificant decimal zeros into a unique
canonical coefficient and exponent, so forms such as `10 kohm` and
`10000 ohm` elaborate identically.

Each component has exactly one kind-appropriate value and `terminals`
declaration and at most one footprint. Declarations are resolved by identity
rather than order. Net, component, pad,
binding, placement, and route collections are canonicalized before the Design
IR is exposed. Every simulated design has exactly one `ground` declaration.
Every physical pad has exactly one explicit logical-pin binding, and every
connected logical pin on a physical component has at least one bound pad.

## CLI and exit status

```sh
bazel run //cmd/circuitc -- compile INPUT \
  --output-dir OUTPUT_DIRECTORY \
  [--diagnostic-format=human|json] [--]
```

The command accepts exactly one input. It writes `<design>.kicad_pcb` and
`<design>.spice` only after parsing, elaboration, Design validation, KiCad
identity validation, and SPICE lowering all succeed.

- `0`: success;
- `1`: source, semantic, or backend-validation failure;
- `2`: invalid invocation or unsupported option; and
- `3`: input or output I/O failure.

Human diagnostics are the default. JSON diagnostics contain stable codes, the
requested filename, UTF-8 byte spans, one-based line and column, semantic path
when available, deterministic messages, and related locations for duplicates.

## Deferred language work

Hierarchy, typed interfaces, explicit no-connects, general library ingestion,
schematic/project generation, new simulation devices and analyses, and
released-language compatibility are owned by EPIC-0002 or later.
