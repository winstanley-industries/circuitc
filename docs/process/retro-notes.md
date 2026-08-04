# Process retrospective notes

## EPIC-0004 APGAR routing integration (2026-08-04)

- The six-layer split was directionally correct: source intent, wire contracts,
  import/projection, real adapter execution, checked compilation, and host
  acceptance each had one reviewable authority. No feedback round required
  moving a fix into another layer, so the mandatory split trigger did not fire.
- PR #16 needed one correction round for checked coordinate arithmetic and
  missing frontend diagnostic coverage. PR #17 needed four request-changes
  verdicts to align request cardinality, bound untrusted resource expansion
  before allocation, and make the regression discriminate the early guard.
- PR #18 needed six request-changes verdicts around simulation-phase
  preservation, exact layer equality, and one-to-one projection failure
  coverage. Several reviews ran on unchanged heads; do not spend another
  convergence run until a new candidate actually contains the promised fix.
- PR #19 needed two correction rounds around process-exit diagnostic precedence
  and real-adapter/process-failure coverage. PR #20 needed one automated round
  plus adversarial follow-up for non-Unix compilation, accepted/provisional
  terminology, post-execution identity checks, and checked routing/simulation
  phase order.
- PR #21 needed four request-changes verdicts before approval. The missing
  ledger rows were reverse PCB-set membership, whitespace-independent KiCad
  structure, unsupported copper forms, provenance rejection paths, canonical
  projection order, exclusive no-follow output publication, and the temporal
  binding between source bytes and the KiCad report.
- Most rework concentrated at external trust boundaries rather than in the
  source/IR slice. CI was largely confirmatory; the longer wall-clock waits were
  exact-head APGAR builds and automated review, while mutation-oriented local
  tests found the same classes quickly once the ledger named them.
- A digest recorded after a host process is not evidence of the bytes that
  process read. Host acceptance must snapshot privately, hash from one open
  handle before execution, recheck identity and digest afterward, and carry
  that pre-execution digest into normalization and the final join.
- Exact evidence joins require reverse as well as forward coverage. For every
  projected collection, enumerate the complete host structure independently,
  reject unsupported sibling forms, and test canonical, alternate-encoding,
  coordinated-extra, and stale-time mutations.
- The durable workflow change is to add two explicit ledger questions for each
  acceptance manifest: "what extra host structure is not projected?" and
  "when were the exact accepted bytes observed by the external authority?"

## EPIC-0002 adversarial remediation (2026-08-01)

- Exercise every accepted IR state through the public source language and the
  supported host, not only through hand-built Rust fixtures. The fixture with
  a physical-only/no-connect component exposed a schematic-to-PCB parity gap
  that unit-level string checks did not.
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
- A manual host gate cannot be the only test for compiler-owned semantics.
  Default tests must independently pin connectivity projection, every
  coordinate-transform branch, and generated project/library configuration.
- Completion evidence must name a reachable repository base or PR commit and
  state when a host-only gate is not reproduced by CI; scratch-repository
  commits are useful local evidence but not durable project provenance.
- Host DRC is meaningful only for geometry actually embedded in the generated
  board. Vendoring courtyard data without lowering it into board footprints
  would leave courtyard-overlap policy vacuous.
- Publication and post-publication cleanup are distinct outcomes. Roll back
  incomplete publication, but report successful publication plus staging
  residue with a dedicated warning instead of claiming the outputs were not
  written.
- A positive source fixture does not prove a fail-closed language rule. Every
  new source diagnostic needs a failure-side assertion for its code, message,
  primary span, and related span where applicable.
- Catalog growth is only additive when compiled artifacts carry an ordered,
  design-derived library-file collection. Fixed symbol or footprint singleton
  fields can silently publish an incomplete project after the second part is
  added.
- Security-relevant path policy needs both an internal regression and a
  process-boundary test that pins the exit code and actionable diagnostic; an
  adjacent symlink test is not evidence for ancestor traversal behavior.
