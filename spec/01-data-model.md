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

A node MUST validate against its declared entity type. Validation uses the
ontology version that was current when the creating or updating changeset
was admitted, so validity is historical: a node that was valid under
`core/Person@2` stays valid after version 3 ships. §2.4 governs migration.

## 1.3 Attributes and the type system

Attribute values are typed. Implementations MUST support:

| Type | Notes |
|---|---|
| `string` | UTF-8. NFC-normalized for hashing |
| `int` / `float` / `decimal` | `decimal` is exact. Use it where drift matters |
| `bool` | |
| `timestamp` / `date` / `duration` | RFC 3339. Timestamps carry offsets |
| `bytes` | Inline below an implementation-declared threshold. Above it, use a document reference |
| `ref` | Typed reference: `node-ref`, `edge-ref`, `document-ref`, or `external-ref` (URI plus optional content hash) |
| `list<T>` / `map<string, T>` | Homogeneous collections |

**Embeddings are not attribute values.** An embedding is model-dependent
derived data, not knowledge. Store an embedding as an `external-ref` to a
derived artifact, tagged with the identity of the model that produced it.
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
| `locator` | uri (optional) | Where the bytes live when not inline. The locator is a retrieval hint. The hash is the identity |

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

Classifications are **data, not annotations**. Changesets create them, they
carry provenance, and governance applies to them. This rule matters because
Part 4 keys policy off classifications: the act of classifying is itself a
governed mutation.

## 1.7 Identifiers and hashing

- **Logical IDs** are UUIDv7 values, minted by the creating principal. They
  are stable across revisions and MUST NOT encode meaning.
- **Revision hashes** are SHA-256 over the object's canonical wire encoding
  (§7.1) with the `rev` field zeroed. Every hash carries an algorithm
  prefix, e.g. `sha256:`. An implementation that encounters an unknown
  algorithm MUST reject the hash rather than guess.
- **The graph state hash** is the root of a Merkle tree over all live
  object revision hashes, grouped by kind and sorted by logical ID. Two
  implementations that fold the same log MUST produce the same state hash.
  The state hash anchors round-trip conformance (§7.4) and replay
  verification (§5.3).

The state hash makes "same graph" a decidable question. A projection that
cannot re-ingest to an identical state hash is lossy by definition, and a
lossy projection MUST declare its losses (§7.2).
