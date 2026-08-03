# Part 3: Changeset Substrates *(L1)*

## 3.1 The abstract substrate interface

Parts 4 to 6 are written against this interface and MUST NOT depend on
any particular substrate. A substrate provides four properties:

1. **Content-addressed revisions.** Every revision has a
   collision-resistant hash identity, derived from its content.
2. **A parent-pointer DAG.** Every revision names its parent revision or
   revisions. History supports branching and merging.
3. **Signable authorship.** Every revision can carry a principal reference
   and a signature over the revision hash.
4. **Deterministic state.** The state at any revision is a pure function
   of the revisions reachable from it. A state hash summarizes it.

This spec defines two substrate bindings: native (§3.2) and git (§3.3).
Other bindings are possible, for example a database WAL. A binding conforms
when it satisfies the four properties.

## 3.2 Native substrate

### 3.2.1 Changeset structure

| Field | Type | Description |
|---|---|---|
| `hash` | hash | SHA-256 of the canonical encoding, with the `hash` and `signature` fields omitted and the operations list represented by its Merkle root (§3.2.6). Appendix H fixes the preimage |
| `parents` | list<hash> | At least one, except genesis. More than one is a merge |
| `author` | principal-ref + key-id | Part 6 |
| `timestamp` | rfc3339 | Author-asserted and informational. Ordering always comes from the DAG |
| `intent` | string (optional) | Rationale for the change, readable by humans and agents. Analogous to a commit message |
| `operations` | list<operation> | §3.2.2. Applied atomically |
| `schema_context` | hash | State hash of the schema the author validated against |
| `signature` | sig \| list<sig> | Over `hash`, by the author's key. A list where a signature threshold applies (§4.6) |

A changeset is atomic. All operations apply, or none do.

Validation always uses the schema at the changeset's first parent.
`schema_context` records the schema the author validated against. When
the two differ, the changeset fails evaluation unless policy explicitly
admits the mismatch, and the admission records it. This closes the
loophole of pinning a stale schema to dodge new constraints.

### 3.2.2 Operation set

The operations are `create`, `update`, and `delete`, each over a `node`,
`edge`, `classification`, or `document`. Each `update` carries the prior
revision hash of its target. This gives optimistic concurrency: a mismatch
at fold time is a conflict (§3.2.4).

Schema mutations are ordinary operations that target schema objects
(§2.5): `define-type`, `deprecate-term`, `set-policy`, and similar. They
travel like any other operation, though policy usually holds them to
stricter review.

`delete` writes a tombstone. The log is append-only and history is never
rewritten. Two governed redaction operations remove content while every
hash and signature stays verifiable. `redact-document` removes a
document's stored bytes and keeps its content hash. `redact-operation`
removes the recorded content of a prior operation, or a changeset's
intent text, and keeps the corresponding leaf or intent hash (§3.2.6).
An object revision produced by a redacted operation keeps its identity,
revision hash, and lineage, and loses its materialized content. §1.8
defines the resulting object state. The removal itself, who performed
it, and under what authority all stay permanently verifiable. This design interacts with erasure law, and
redaction now reaches attributes and intent text as well as document
bytes. See threat T8 in Appendix E.

### 3.2.3 Ordering and merges

A changeset with multiple parents merges branches. When two branches
mutated disjoint objects, the merge is automatic. When they touched the
same object:

- Conflicting revisions require explicit resolution. The spec treats
  silent overwrite of knowledge as data loss and defines no
  last-writer-wins default.
- The merge changeset MUST include explicit `resolve` operations. Each
  `resolve` chooses or synthesizes the surviving revision.
- At L2, `resolve` operations are policy-visible. A graph can require
  human review when branches conflict about a `sensitivity/private`
  subject.

### 3.2.4 Fold semantics

The state at revision R is the topological fold of all changesets
reachable from R. At fold time, implementations MUST reject three things:
results that violate the schema, dangling references, and `update`
operations whose prior-revision hash does not match. A merge with
`resolve` operations clears the third case. Failures attribute to the
first revision at which they appear: a merge whose combined result
violates the schema or dangles a reference is itself the rejected
changeset, and the merge author must add operations that repair the
result. A rejected changeset is skipped and flagged, and the log folds
past it. Rejection propagates through dependence, not proximity: a
later changeset whose operations target revisions or reference objects
the rejected changeset produced fails the same checks in turn, while
independent changesets fold normally.

### 3.2.5 Checkpoints

A checkpoint is a signed triple: revision hash, state hash, and a full
state projection. Checkpoints allow cold-start without full replay. A
checkpoint is an optimization. Replay MUST be able to verify it, and a
checkpoint that disagrees with replay is invalid.

**Anchoring.** A graph SHOULD periodically publish signed checkpoint
references to at least one external witness: a peer graph (§9.3), a git
repository, a transparency log, or any host outside the owner's sole
control. Anchors bound when a revision existed, and they turn
equivocation into a provable event: two anchors that bind the same
revision to different state hashes convict the equivocator. Anchors
also give governance audits an external time bracket (§4.2).

### 3.2.6 Operation Merkle tree and elision

For hashing, the `operations` list is summarized as a Merkle tree: each
leaf is the hash of one operation's canonical encoding, in list order,
and leaf and interior hashes carry domain-separated prefixes. The
changeset `hash` covers this root in place of the raw list, so the wire
form can carry all operations, a subset, or none while the hash and
signature stay verifiable. The canonical encoding also covers the
`intent` field by its hash, so `redact-operation` (§3.2.2) can remove
intent text with the chain intact.

An **elided changeset** replaces undisclosed operations with their leaf
hashes and tree positions. A verifier can confirm authorship,
parentage, and the membership of every disclosed operation. A fold over
elided history produces the disclosed subset of state. Share bundles
(§9.5) pair elided history with subgraph proofs so a receiver can also
check that the subset is consistent with the source's full state.

Elision and redaction solve different problems. `redact-document`
(§3.2.2) removes stored bytes from the graph itself. Elision keeps the
source graph complete and controls what a disclosure reveals.

## 3.3 Git substrate binding

Git already satisfies the §3.1 interface:

| Interface requirement | Git mechanism |
|---|---|
| Content-addressed revisions | Commit SHA (object model) |
| Parent DAG | Commit parents |
| Signable authorship | Commit and tag signing: GPG, SSH, Sigstore |
| Deterministic state | Tree hash |

### What the binding adds

The binding maps git onto Allod's vocabulary so that Parts 4 to 6 apply. A
commit is a changeset. Its operations are its file diff. The tree hash is
the state hash. Refs are branch heads. Commit signatures are authorship.

For policy evaluation, a commit's operation set is computed by
byte-level tree comparison against the first parent, with rename
detection disabled. Two evaluators MUST derive identical operation sets
from the same commit. Rename heuristics vary across tools, and a rule
match must never depend on one.

Governance policy for a git substrate keys on repo, path, and branch
patterns instead of taxonomy terms (§4.1). CODEOWNERS and branch
protection are special cases of this rule shape, but both are host-locked
and unsigned. The binding expresses the same rules in a portable,
verifiable form.

### What the binding does not do

Allod never stores, re-encodes, or mirrors code. Git stays authoritative
for content and for its own merge semantics. Allod adds the layers the git
ecosystem lacks: portable signed decision records, policy evaluation, and
attestation. In conformance terms, git natively provides L0 and L1 but has
never had a portable L2 or L3. The binding exists to fill that gap.

### Known limitation

A git remote will advance a ref without any decision record, so a git
substrate can hold state that never passed admission. A native substrate
at L2-enforced holds only admitted state. Git substrates therefore
support L2-observed
everywhere, and L2-enforced only where the deployment controls the remote
through a ref gate, a merge queue, or a pre-receive hook. §4.5 defines
both strengths.

## 3.4 Cross-substrate references

A node in a native graph MAY reference a revision in another substrate
or an object in another Allod graph (§9.2 defines the `allod:` reference
form). Example: a `document` whose locator is
`git:<repo-url>#<commit-sha>:<path>`. A decision record in a native
graph MAY govern a git changeset. Three rules apply:

1. A cross-substrate reference MUST carry the target's content hash,
   which identifies the target. The URL only says where a copy may be
   found, and it can go stale.
2. Reference integrity is eventual. The native substrate cannot stop a
   git remote from garbage-collecting a commit. When a verifier cannot
   resolve a cross-reference, it reports the result as degraded. The
   claim stands, and only its evidence is offline.
3. The derived-graph machinery (§8.3) generates cross-substrate
   references systematically and inherits these rules.
