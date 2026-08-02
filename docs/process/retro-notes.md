# Process retrospective notes

## EPIC-0002 adversarial remediation (2026-08-01)

- Exercise every accepted IR state through the public source language and the
  supported host, not only through hand-built Rust fixtures. The physical-only
  no-connect fixture exposed a schematic-to-PCB parity gap that unit-level
  string checks did not.
- Tests for nested formats must inspect the complete owning stanza or parse the
  structure. Matching only an opening line can miss absent child semantics.
- Treat rendered semantic paths as a global namespace independently of UUID
  uniqueness. Both are identity-manifest contracts and both need adversarial
  collision tests.
- Derive deterministic provenance labels from canonical design identity. Keep
  requested filenames for diagnostics, but never allow absolute paths or
  invocation spelling into artifacts.
- Model part support as one exact catalog tuple. Validating manufacturer,
  logical device, symbol, and footprint independently permits incoherent but
  individually known combinations.
- Host-report policy is data: require the exact severity set and explicit
  ignored-check allowlists, aggregate every finding, and attach source identity
  before failing.
- Host claims must match real CLI surfaces. KiCad parses the s-expression
  artifacts and owns ERC/DRC/parity; CircuitC structurally validates the exact
  generated project JSON because KiCad 10 exposes no direct project parser.
- Output containment must be tested with hostile symlinked parents and external
  sentinels before any destination is touched.
- Filesystem containment starts at the first existing ancestor, including
  output-root creation. Pin directories with no-follow handles and perform
  traversal, publication, and rollback descriptor-relatively; preflight path
  checks alone always retain swap races.
- Treat rollback as part of the transaction result. Cleanup or restoration
  failures must be accumulated and surfaced, with fault-injection tests proving
  both original restoration and staging-residue removal.
- Exact generated subsets require exact validators. Type-checking project JSON
  containers or accepting best-effort identity joins can turn malformed
  evidence into apparent success.
- Keep provenance lookup costs explicit. Pre-index rendered semantic paths,
  line starts, and UTF-8 character starts so large or minified sources do not
  make identity-manifest generation quadratic.
