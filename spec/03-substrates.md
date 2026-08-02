# Part 3 — Changeset Substrates *(L1)*

## 3.1 The abstract substrate interface

Parts 4 to 6 are written against this interface. They MUST NOT depend on
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
| `hash` | hash | SHA-256 of the canonical encoding, with the signature zeroed |
| `parents` | list<hash> | At least one, except genesis. More than one is a merge |
| `author` | principal-ref + key-id | Part 6 |
| `timestamp` | rfc3339 | Author-asserted. The DAG is the ordering authority. The clock never is |
| `intent` | string (optional) | Human-readable and agent-readable rationale. Like a commit message |
| `operations` | list<operation> | §3.2.2. Applied atomically |
| `schema_context` | hash | State hash of the schema the author validated against |
| `signature` | sig | Over `hash`, by the author's key |

A changeset is atomic. All operations apply, or none do.

### 3.2.2 Operation set

The operations are `create`, `update`, and `delete`, each over a `node`,
`edge`, `classification`, or `document`. Each `update` carries the prior
revision hash of its target. This gives optimistic concurrency: a mismatch
at fold time is a conflict (§3.2.4).

Schema mutations are ordinary operations that target schema objects
(§2.5): `define-type`, `deprecate-term`, `set-policy`, and similar. They
get no special transport. They usually get stricter policy.

`delete` writes a tombstone. The log is append-only. History is never
rewritten. To remove genuinely toxic content, use the explicit, governed
`redact-document` operation. It removes stored bytes and preserves the
hash chain. The fact of the removal, who did it, and under what authority
stay permanently verifiable. Erasure law interacts here. See threat T8 in
Appendix E.

### 3.2.3 Ordering and merges

A changeset with multiple parents merges branches. When two branches
mutated disjoint objects, the merge is automatic. When they touched the
same object:

- There is **no last-writer-wins default.** The spec treats silent
  overwrite of knowledge as data loss.
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
`resolve` operations clears the third case. A rejected changeset poisons
nothing. It is not part of any valid state. Consumers skip it and flag it.

### 3.2.5 Checkpoints

A checkpoint is a signed triple: revision hash, state hash, and a full
state projection. Checkpoints allow cold-start without full replay. A
checkpoint is an optimization. Replay MUST be able to verify it. A
checkpoint that disagrees with replay is invalid.

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

Governance policy for a git substrate keys on repo, path, and branch
patterns instead of taxonomy terms (§4.1). CODEOWNERS and branch
protection are special cases of this rule shape. They are host-locked and
unsigned. The binding makes the same rules portable and verifiable.

### What the binding does not do

Allod never stores, re-encodes, or mirrors code. Git stays authoritative
for content and for its own merge semantics. Allod adds the layers the git
ecosystem lacks: portable signed decision records, policy evaluation, and
attestation. In conformance terms: git ships L0 and L1 natively. It has
never had a portable L2 or L3. That gap is the binding's purpose.

### Known limitation

A git remote will advance a ref with no decision record at all. A
native-substrate L2-enforced graph cannot contain unadmitted state. A git
substrate can. Git substrates therefore support L2-observed everywhere.
They support L2-enforced only where the deployment controls the remote,
through a ref gate, a merge queue, or a pre-receive hook. §4.5 defines
both strengths.

## 3.4 Cross-substrate references

A node in a native graph MAY reference a revision in another substrate.
Example: a `document` whose locator is `git:<repo-url>#<commit-sha>:<path>`.
A decision record in a native graph MAY govern a git changeset. Three
rules apply:

1. A cross-substrate reference MUST carry the target's content hash. A
   URL is a hint. The hash is the identity.
2. Reference integrity is eventual, not transactional. The native
   substrate cannot stop a git remote from garbage-collecting a commit.
   When a verifier cannot resolve a cross-reference, it reports the
   result as degraded, not invalid. The claim stands. Its evidence is
   offline.
3. The derived-graph machinery (§8.3) generates cross-substrate
   references systematically and inherits these rules.
