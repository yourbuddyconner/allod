# Part 1: Core Data Model *(L0)*

## 1.1 Overview

A graph is the set `{nodes, edges, classifications, documents}` produced by
folding a changeset log (Part 3) against a schema (Part 2). This part
defines the objects. Part 3 defines how they change. Part 7 defines how
they are encoded.

All four object kinds share a common envelope:

| Field | Type | Description |
|---|---|---|
| `id` | logical-id | Stable identity across revisions (§1.7) |
| `rev` | hash | Content hash of this revision of the object |
| `kind` | enum | `node` \| `edge` \| `classification` \| `document` |
| `provenance` | lineage-ref | REQUIRED at L1 and above. See §5.1 |

## 1.2 Node

A node is an entity: a person, a company, a concept, a function, a
decision.

| Field | Type | Description |
|---|---|---|
| `type` | ontology-type-ref | Entity type plus ontology version, e.g. `core/Person@2` |
| `attributes` | map<name, typed-value> | Per §1.3. Validated against the type's attribute schema |

A node MUST validate against its declared entity type. Validation uses
the schema in force at the changeset's parent revision, which the
changeset pins as `schema_context` (§3.2.1). A node that was valid under
`core/Person@2` therefore stays valid after version 3 ships. §2.4
governs migration.

## 1.3 Attributes and the type system

Attribute values are typed. Implementations MUST support:

| Type | Notes |
|---|---|
| `string` | UTF-8. NFC-normalized for hashing |
| `int` / `float` / `decimal` | `decimal` is exact. Use it where drift matters. A `float` MUST be finite, and encoders normalize negative zero to zero, so hashes stay stable across platforms |
| `bool` | |
| `timestamp` / `date` / `duration` | RFC 3339. Timestamps carry offsets |
| `bytes` | Inline below an implementation-declared threshold. Above it, use a document reference |
| `ref` | Typed reference: `node-ref`, `edge-ref`, `document-ref`, or `external-ref` (URI plus optional content hash) |
| `list<T>` / `map<string, T>` | Homogeneous collections |
| `enum<a\|b\|…>` | A closed set of string symbols, declared in the type expression. Symbols use `[A-Za-z0-9._-]`. A value hashes as its string |
| `selector` | A policy-selector expression (§4.1 grammar). Grants and delegation scopes (§6.1, §9.4) use it, so the objects that scope authority carry the same machine-checkable form that policy rules do |
| struct name | A record type declared by an ontology (§2.1). A struct value is validated structurally and has no identity of its own |

Embeddings do not belong in attribute values. An embedding depends on the
model that generated it, so it is derived data rather than stable
knowledge. Store an embedding as an `external-ref` to a derived artifact,
tagged with the model's identity.
Graphs then stay meaningful across model generations. Implementations MAY
keep embedding side-tables in projections such as Parquet (§7.3).

## 1.4 Edge

Edges are first-class objects. Each edge has its own identity and
provenance, and an edge can be classified.

| Field | Type | Description |
|---|---|---|
| `type` | ontology-edge-ref | Edge type plus ontology version, e.g. `core/employs@1` |
| `from` / `to` | node-ref | Direction is semantic, per the edge type's definition |
| `attributes` | map | Edge attributes, e.g. `since`, `confidence`. Schema-validated |

An edge MUST satisfy the domain and range constraints of its type (§2.1) at
admission time. Dangling references are a substrate concern: the native
substrate rejects changesets that produce them (§3.2.4), and
cross-substrate references follow §3.4.

## 1.5 Document

A document is a source artifact that knowledge derives from: a file, a
message, a meeting transcript, a commit, a web page.

| Field | Type | Description |
|---|---|---|
| `content_hash` | hash | REQUIRED. Hash of the exact bytes |
| `media_type` | string | RFC 6838 |
| `storage` | enum | `inline` \| `stored` \| `external` |
| `locator` | uri (optional) | Where the bytes can be fetched when not inline. Identity always comes from `content_hash`, so a stale locator does no harm |

A graph MAY store document bytes or only reference them. Lineage
verification (§5.3) degrades gracefully. With the stored bytes, a verifier
can re-derive the claim. With only the hash, a verifier can still confirm
that a presented copy of the document matches the bytes the claim was
derived from.

## 1.6 Classification

A classification assigns a subject to a taxonomy term:

| Field | Type | Description |
|---|---|---|
| `subject` | node-ref \| edge-ref \| document-ref | What is classified |
| `term` | taxonomy-term-ref | Term plus taxonomy version, e.g. `sensitivity/private@3` |
| `asserted_by` | principal-ref | Who or what made this classification |
| `basis` | enum | `manual` \| `deterministic` \| `model-assisted` (§8.2) |

Classifications are full objects with the same lifecycle as nodes and
edges: changesets create them, they carry provenance, and governance
applies to them. This matters because Part 4 keys policy off
classifications, so the act of classifying is itself a governed mutation.

## 1.7 Identifiers and hashing

- **Logical IDs** are UUIDv4 values, minted by the creating principal.
  They are stable across revisions and MUST NOT encode meaning. Full
  randomness is the point of version 4: a timestamp-bearing ID such as
  UUIDv7 leaks creation time through every reference and survives
  redaction.
- **Revision hashes** are SHA-256 over the object's canonical wire encoding
  (§7.1) with the `rev` field omitted, under a domain-separated
  prefix. Every hash carries an algorithm
  prefix, e.g. `sha256:`. An implementation that encounters an unknown
  algorithm MUST reject the hash rather than guess. The Appendix H
  vectors fix the exact preimages.
- **The graph state hash** is the root of a Merkle tree over all live
  object revision hashes and the tombstones of deleted objects, grouped
  by kind and sorted by logical ID. The tree is binary, built over the
  sorted leaves, with an odd node promoted unchanged. Leaf and interior
  hashes carry domain-separated prefixes. Two implementations that fold
  the same log
  MUST produce the same state hash. The state hash anchors round-trip
  conformance (§7.4), replay verification (§5.3), and the subgraph
  proofs that federation shares are built on (§5.4, §9.5).

The state hash gives a definite test for whether two copies are the same
graph. A projection that cannot re-ingest to an identical state hash is
lossy by definition, and a lossy projection MUST declare its losses
(§7.2).

## 1.8 Object states

At any revision, each object is in exactly one state:

- **Live.** The current revision materializes in full. This is the
  normal state.
- **Deleted.** A `delete` tombstone ended the object (§3.2.2). Identity
  and history remain, the tombstone appears in the state tree (§1.7),
  and the object is absent from projections of current state.
- **Redacted.** Redaction removed the content behind the object's
  current revision (§3.2.2). The envelope survives: `id`, `rev`, `kind`,
  and `provenance` remain, and attributes or document bytes are absent.
  The object stays in the state tree under its recorded revision hash.

Rules:

- Redaction of a historical revision leaves the object live. Only the
  recorded content of that revision is gone, and the current revision
  still materializes.
- References to a redacted or deleted object stay intact, because
  identity and revision hashes persist. Redaction never creates a
  dangling reference.
- Classifications on a redacted subject persist, and lineage that cites
  redacted content stays tombstone-marked (§8.4).
- For an edge, `type`, `from`, and `to` are envelope rather than
  content: the state tree and reference integrity depend on them, so
  they survive redaction. Redacting an edge removes its attributes
  only. The existence of the relationship stays visible, and where
  that existence is itself the sensitive fact, redaction cannot erase
  it. Threat T8 carries this limit.
- A recorded hash whose content was redacted can no longer be
  recomputed. Integrity verification reports it as degraded rather than
  verified (§5.3), which separates content removed under authority from
  content that is merely missing.
