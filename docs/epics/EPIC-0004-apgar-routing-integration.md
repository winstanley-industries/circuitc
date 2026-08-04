# EPIC-0004: APGAR routing integration

- Status: Complete
- Architecture milestone: M3
- Depends on: EPIC-0002

## Outcome

CircuitC lowers exact placed-board intent into a versioned APGAR request,
receives deterministic route candidates with provenance, imports a selected
route without identity loss, and requires both APGAR exact validation and
KiCad DRC before accepting it.

## Requirements

| ID | Requirement |
| --- | --- |
| `CC-REQ-ROUTE-001` | The CircuitC-to-APGAR request and result contracts are versioned, checksummed, CAD-neutral, and independent of APGAR's internal C++ or CUDA layouts. |
| `CC-REQ-ROUTE-002` | Coordinate, layer, connectivity, obstacle, and rule lowering is exact and uses checked conversion between CircuitC nanometres and APGAR database units. |
| `CC-REQ-ROUTE-003` | Route candidates carry stable identity, request identity, toolchain identity, cost data, validation status, and replay provenance. |
| `CC-REQ-ROUTE-004` | A deterministic CPU reference path establishes correctness before GPU performance work is accepted. |
| `CC-REQ-ROUTE-005` | Selected routes import into canonical physical intent and deterministic KiCad output without silently weakening unsupported geometry or rules. |
| `CC-REQ-ROUTE-006` | APGAR exact validation and supported KiCad DRC both gate every accepted imported route. |
| `CC-REQ-ROUTE-007` | Failed, unsupported, stale, or mismatched requests and results produce machine-readable diagnostics and cannot be imported. |

## Initial capability boundary

- Source-authored `route` segments remain canonical copper. The distinct
  `autoroute` form authors one unresolved request and cannot be emitted as
  copper before authenticated import.
- Design IR v1 permits zero or one autoroute request for exactly one
  two-terminal physical net on one selected front or back layer. The CPU
  reference supports horizontal, vertical, and 45-degree centreline segments.
- The initial contract excludes vias, arcs, multipin routing, other headings,
  and approximation of unsupported geometry or rules.
- CircuitC crosses to APGAR's current database-unit domain with checked
  nanometre multiplication by two. Strict canonical versioned JSON request and
  result contracts authenticate the selected result before lossless import.
- Imported geometry becomes canonical physical intent only after
  authentication and validation. APGAR exact admission and supported KiCad DRC
  remain separate required acceptance authorities.

## Non-goals

- Reimplementing APGAR inside CircuitC.
- Sharing mutable in-process implementation objects across the contract.
- GPU optimization before deterministic CPU correctness and replay exist.
- Treating APGAR validation as a substitute for KiCad DRC.

## Acceptance gates

- A checked-in deterministic CPU fixture round-trips through request, route,
  import, APGAR validation, KiCad emission, and KiCad DRC.
- Repeated requests and CPU results are byte-identical.
- Contract corruption, version mismatch, coordinate overflow, unsupported
  rules, and stale provenance are rejected by exact diagnostics.
- GPU results, when enabled, reproduce valid CPU-reference semantics within the
  declared routing contract.
- All repository Bazel and strict lockfile gates pass.

## Completion evidence

The dependency-ordered implementation stack merged through pull requests #16,
#17, #18, #19, #20, and #21. The final implementation commit on `main` is
`063154e9f8fd4e317432a066a66a844ff0606d6b`; its tree
`cca8dd029d885486d0562bf5cd096f279c07022c` exactly matches reviewed PR #21
head `8be2593f015671103d9aae152caac627fb8ce13a`. All six pull requests are
merged and have zero unresolved review conversations.

| Requirement | Merged implementation and discriminating evidence |
| --- | --- |
| `CC-REQ-ROUTE-001` | PR #17 added strict canonical request/result schemas and Rust parsers with version, exact key, field-order, digest, association, collection-bound, and corruption tests. PRs #18 and #21 added separately versioned projection and acceptance manifests; reordered projection objects fail at the acceptance join. |
| `CC-REQ-ROUTE-002` | PRs #16 and #17 added exact Design IR/source intent and checked nanometre-to-DBU multiplication by two, selected-layer/net/terminal lowering, conservative pad/authored-copper obstacles, grid/ROI checks, and overflow diagnostics. `//:circuitc_test` covers success, boundary, permutation, overflow, and unsupported cases. |
| `CC-REQ-ROUTE-003` | PRs #17 through #19 bind stable request, candidate, replay, policy, resource, geometry, payload, tool, executable, source-revision, batch, and query identities. Import independently reconstructs candidate evidence and rejects stale, forged, mismatched, or noncanonical provenance. |
| `CC-REQ-ROUTE-004` | PR #19 builds the checksum-pinned APGAR CPU adapter from APGAR's public Board IR, geometry compiler, CPU A-star, candidate construction, and exact-admission APIs. Real-adapter tests cover horizontal, vertical, diagonal, obstacle-detour, no-route, repeat-byte, process-limit, and diagnostic behavior. No GPU route producer is enabled or claimed by this completion. |
| `CC-REQ-ROUTE-005` | PRs #18 and #20 authenticate and losslessly import the selected H/V/45 one-layer geometry into a fresh validated Design IR, derive stable route UUIDs, emit deterministic provisional KiCad artifacts, and reject layer, identity, path, cardinality, odd-DBU, unsupported-geometry, or simulation-phase drift. |
| `CC-REQ-ROUTE-006` | PR #21 added the strict Rust evidence verifier, private immutable KiCad project snapshot runner, normalized pre/post source-digest binding, exact bidirectional PCB segment-set/geometry checks, and the canonical acceptance manifest. `//:route_acceptance_manifest_test` and the KiCad 10.0.5 `//:kicad10_drc_test` close APGAR exact admission and clean ERC/DRC/unconnected/parity over the same artifacts. |
| `CC-REQ-ROUTE-007` | Every layer adds stable machine-readable diagnostics and mutation tests. The final acceptance suite rejects corrupt contracts, stale associations, unadmitted or forged candidates, substituted provenance, noncanonical projection order, changed artifacts, stale host reports, unexpected findings/ignored checks, extra or alternate-format segments, unsupported arc/via/zone copper, symlinked output paths, and existing outputs. |

On macOS arm64 with Bazel 9.2.0, a clean worktree at merged commit
`063154e9f8fd4e317432a066a66a844ff0606d6b` passed:

- `bazel lint //...`
- `bazel build //...`
- `bazel build --lockfile_mode=error //...`
- uncached ordinary and strict-lockfile `bazel test //...` runs, each with all
  15 test targets passing
- `bazel mod graph --lockfile_mode=error`
- uncached supported-host `//:kicad10_drc_test` with error output against
  KiCad 10.0.5

The host gate compiled two independent routed fixtures byte-identically, ran
KiCad ERC and schematic-parity DRC against private immutable snapshots, bound
the normalized reports to the exact pre-execution source digests, emitted
identical acceptance manifests, and rejected a board changed after DRC. The
host gate is intentionally `local` and `manual`; Linux CI does not provide the
supported KiCad executable. PR-event GitHub Actions run `30896135847` passed
Linux, workflow-security, and the exact-head Claude approval on
`8be2593f015671103d9aae152caac627fb8ce13a`. Post-merge push run
`30896759963`, bound directly to `063154e9f8fd4e317432a066a66a844ff0606d6b`,
passed Linux and workflow-security; its Claude job was intentionally skipped
because automated review is a pull-request gate.
