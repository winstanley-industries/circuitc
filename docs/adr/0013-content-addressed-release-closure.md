# ADR-0013: Publish an independently verified, content-addressed release closure

- Status: Accepted
- Date: 2026-08-04

## Context

Layers 1 through 5 of EPIC-0005 establish authoritative product intent,
authenticated offline catalog evidence, deterministic product projections,
normalized KiCad fabrication output, and capability-specific KiCad board
analysis. They do not prove that one distributable directory contains the
complete and mutually consistent closure of those artifacts. They also do not
bind the exact CircuitC source, the complete Design IR semantics, checked
simulation or routing evidence when applicable, or the exact tool images used
to create the release.

A caller-authored checksum list is insufficient. Such a list can be internally
consistent while omitting a required predecessor, joining artifacts from
different Designs, or declaring a dirty analysis successful. Publication also
cannot be a sequence of independently visible file replacements: a crash or
race could expose a partial release or overwrite an immutable predecessor.

## Decision

Layer 6 adds strict `circuitc.release_request` and
`circuitc.release_manifest` v1 contracts. The Rust release binder consumes
typed predecessor bundles and exact tool bytes. It independently reconstructs
and verifies the authoritative joins before exposing release bytes:

- exact UTF-8 CircuitC source is elaborated again and must equal the supplied
  valid Design;
- the exact catalog snapshot, variant, and four Layer-3 product artifacts are
  reverified;
- the exact Layer-4 fabrication request, manifest, and normalized inventory
  are reverified against current Design, product, compiler, KiCad executable,
  and raw host evidence;
- the exact Layer-5 request, result, report, ERC evidence, and DRC evidence are
  reverified, and the recomputed report must be completed with every declared
  capability passing;
- every declared simulation contributes its checked netlist, request, identity
  map, result, and assertion report, and requires exact Ohmnivore tool evidence;
  simulation evidence and the tool are forbidden when no simulation is
  declared; and
- a declared APGAR routing request contributes its checked request, result,
  projection, and accepted route-manifest evidence, and requires exact APGAR
  executable and provenance bytes. Source-authored route segments alone do not
  make APGAR applicable, and routing evidence is forbidden when no routing
  request is declared.

The inventory is constructed from those typed in-memory values. Callers cannot
add arbitrary paths, omit applicable files, or make a directory scan define
the release.

## Source and Design identities

Source identity binds its derived path, exact byte length, and lowercase
SHA-256. Comments and whitespace therefore remain release-significant even
when they elaborate to the same Design.

Design identity is lowercase SHA-256 over ASCII
`CIRCUITC-DESIGN-IDENTITY-V1`, one NUL byte, and a private, typed,
length-delimited encoding of the complete canonical Design. The encoder visits
every Design v1 field in schema order after validation and canonicalization.
It uses big-endian fixed-width integers, one-byte closed-enum tags, explicit
option tags, and length-prefixed UTF-8 strings and collections. Exact quantities
encode their canonical signed coefficient, exponent, and unit tag. This
preimage is an identity encoding, not a public Design serialization contract.
Source locations and frontend provenance spans remain outside it.

## Request, manifest, and release identity

Every release file occupies one safe ASCII relative suffix beneath
`release/<release_identity_sha256>/`. Artifact suffixes are derived from typed
roles and predecessor paths. The release request binds source and Design
identities, variant and catalog identities, applicability, exact tool
identities, the complete sorted artifact inventory, and fixed resource policy.

`release_identity_sha256` is domain-separated SHA-256 over ASCII
`CIRCUITC-RELEASE-IDENTITY-V1`, one NUL byte, and the compact canonical JSON
encoding of the request identity preimage without a final LF. The request then
contains that identity. The manifest binds the exact request and repeats the
verified source, Design, applicability, tools, validations, and inventory. It
sets `all_pass` true only after all current-input verification succeeds.

The manifest cannot include its own digest without a cycle. Its fixed path and
bytes are derived from the request and independently reconstructed by the
verifier. A later materializer treats `manifest.json` as the completion
sentinel and writes it last in private staging.

## Tool identities

Every tool row is derived from exact bytes retained by the binder and contains
its closed role, exact expected version or source revision, byte length, and
SHA-256. KiCad 10.0.5 is required and must match both fabrication and analysis
execution receipts; the exact committed analysis normalizer and host-runner
digests are retained. Ohmnivore is present exactly when simulation applies,
and its executable digest must equal the execution identity retained by every
checked simulation. APGAR is present exactly when routing applies, and its
executable and provenance must equal the checked result, projection, and strict
route acceptance. Caller-authored digest or version strings are not authority.

CircuitC and Bazel do not yet emit an execution receipt that can authenticate
their own image. Layer 6 therefore does not mislabel arbitrary caller bytes as
those tools. Exact source, complete Design identity, deterministic compiler
artifacts, release-contract version, lockfile-error gates, and clean-checkout
reproduction remain their current evidence. A future compiler/build provenance
contract may add closed tool rows without weakening v1 acceptance.

## Inventory and resource policy

Paths are nonempty portable ASCII relative paths made only of normal
components. Absolute paths, empty components, `.`, `..`, backslashes, NUL,
platform prefixes, hidden transaction names, and reserved request or manifest
paths are rejected. Exact duplicates, ASCII-case-fold collisions, and
file/directory prefix collisions are rejected. Each path is at most 4096 bytes
before case folding. Roles are closed and unique where their semantics require
uniqueness.

Every ordinary input and artifact is limited to 67,108,864 bytes. At most 4096
release files and 1,048,576 aggregate path bytes are permitted. Checked
consumed-input and emitted-release aggregate limits are each 1,073,741,824
bytes. The larger aggregate is intentional because one release closes multiple
individually bounded predecessor layers. Limit failure exposes no partial
bundle.

## Independent verification

The verifier reconstructs the expected request, identity, manifest,
applicability, tools, paths, lengths, digests, and exact bytes from current
authoritative inputs. It does not accept mutually consistent rewrites of a
bundle. A successfully verified bundle owns its exact bytes; publication never
re-reads caller paths.

## Layer boundary

Layer 6 creates and verifies one opaque in-memory `ReleaseBundle`. It does not
choose a destination, enumerate ambient directories, run a host tool, publish
files, or claim that a filesystem transaction succeeded. The bundle owns every
exact byte and exposes only its derived content-addressed root and complete
typed inventory.

Layer 7 may consume only a `VerifiedReleaseBundle` produced by independent
Layer-6 reconstruction. It owns secure no-follow filesystem access, private
same-filesystem staging, exact-tree materialization, file and directory fsync,
atomic no-replace publication, cleanup, post-publication verification, and any
future archive, signing, upload, or registry boundary. Existing destinations
remain immutable, including byte-identical releases.

## Transactional materialization

The Layer-7 API separates destination authority from release authority. A
`ReleaseDestination` is acquired only by descriptor-relative, no-follow
walking of an absolute path. Namespace ancestors must be root- or
caller-owned and may not be non-sticky group/world-writable or grant a
permissive Darwin extended ACL. The retained final descriptor must be
caller-owned, mode 0700, and free of extended ACLs. Publication never resolves
that pathname again.

The effective UID that owns this private destination is the publication
authority and the security-principal boundary. The transaction excludes other
UIDs and remains correct under cooperating concurrent publisher calls. POSIX
ownership and mode bits cannot isolate malicious code running with the same
effective UID: that code can chmod, rename, or replace caller-owned files both
before and after any syscall boundary. Such code is therefore outside this
filesystem threat model; callers that require isolation from it must publish
under a separate service account or stronger operating-system sandbox. The
publisher still rechecks names, device/inode identities, metadata, inventory,
and bytes at the last available pre-rename point and after visibility, but does
not misrepresent those observations as protection from its own owner.

`publish_release` accepts that descriptor capability and only an opaque
`VerifiedReleaseBundle`. Beneath a private mode-0700 `release` container it
creates a random private sibling transaction on the same filesystem. Files
are created with no-follow and exclusive-create semantics, changed to mode
0400, and synchronized individually. Directories are changed to mode 0500 and
synchronized bottom-up. `request.json` is first and `manifest.json` is the
last file. Immediately before visibility, the publisher reacquires the staging
root through the held parent, rechecks its device/inode identity, and verifies
the complete tree and exact bytes.

Visibility is one operating-system no-replace rename of the transaction root
to the lowercase release SHA-256. A pre-existing destination of any type,
including a byte-identical directory, is never opened for repair or replaced.
Because some network filesystems can report a transport failure after the
server committed a rename, every rename error is reconciled against the held
staging device/inode and both source and destination names. If the staging
identity is already visible and its old name is absent, publication continues
through sync and exact verification with an explicit ambiguous-rename warning;
it may not return a pre-visibility error for an already visible root.
If neither the visible name nor the staging name can be conclusively joined to
the retained identity, the API returns `VisibilityIndeterminate`, preserves all
possible residue, synchronizes the parent, and attempts visible-tree
verification. Callers must reconcile that explicit state; it is neither a
publication success nor authority to retry destructively.
A staging-name identity match authorizes cleanup only when a successful
no-follow metadata probe observes a different visible-name inode, or the
rename syscall itself was rejected as unsupported before filesystem execution.
Any visible-name lookup error, including a possibly cached negative `ENOENT`,
wins over a possibly stale positive source lookup and produces the
indeterminate state. Cleanup through the retained descriptor could otherwise
hollow out a release that the server already made visible.
Before that rename, failure cleanup atomically renames each known entry to a
random private cleanup claim and removes it only if the claimed device/inode
identity still matches; changed or unknown residue is restored or preserved
and reported for recovery. After the rename, the parent directory is
synchronized and the visible name, root identity, complete inventory, modes,
link counts, lengths, and bytes are reverified. A post-rename failure returns
`PublishedWithWarnings`; because visibility already occurred, it never rolls
back or rewrites the immutable root.

## Consequences

- One release identity proves one exact source and complete verified artifact
  closure; changed bytes create a new immutable release.
- Static, simulation, routed, and combined Designs have explicit and
  fail-closed applicability rather than heuristic inventory.
- The release manifest does not replace predecessor verification; it records
  the result of rerunning it.
- Layer 6 is host-path-free; Layer 7 can give publication one visibility point
  without changing release identity semantics.
- The release SHA-256 is an immutable namespace key, not an idempotent update
  key. Retrying an already visible identity returns an existing-destination
  error even when every byte is equal.
- The Design identity encoder must be updated whenever Design v1 gains a field;
  mutation coverage makes omissions a release-contract failure.
