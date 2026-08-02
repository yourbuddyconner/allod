# Allod

**An open format for sovereign knowledge graphs.**

> Working name. "Allod" (allodial title) is the legal term for property that
> the holder owns absolutely, free of any landlord or superior authority.
> This spec applies that goal to knowledge. You own the graph outright. Its
> history is verifiable. Changes follow rules that you declare.

| | |
|---|---|
| **Status** | Draft v0.3. For discussion. Not implementation-stable. |
| **Author** | Conner Swann |
| **Date** | 2026-08-02 |
| **Supersedes** | The unnamed "Open Knowledge Format" concept notes (2026-08-02) |

## Abstract

Allod defines a format for knowledge graphs with three properties:

- **Sovereign.** The owner controls the graph. No capability depends on a
  host or vendor.
- **Verifiable.** Anyone can rebuild and check every state from signed
  history.
- **Governed.** Declared, machine-evaluable policy controls how the graph
  changes.

Allod specifies how a graph records and admits changes. A graph is stored
as an append-only log of signed changesets. To read the graph, an implementation replays the
log and builds the current state, the same way git builds a working tree
from commits. Each changeset is validated against a versioned schema as it
is applied. Markdown bundles, Parquet tables, and wire messages are
exports of that state, and an implementation can regenerate them at any
time. The log is the only source of truth.

Because the rules live on the changes rather than on the content,
governance, provenance, and attestation work on anything that has a log of
the right shape. The spec defines a native log and also treats a git
repository as one, so the same policy and audit machinery applies to a
knowledge graph and to a codebase.

Provenance that survives transfer is what makes federation workable, and
Part 9 builds on that property. Graphs identify each other by genesis
hash, disclose scoped, verifiable slices of history under signed grants,
and import each other's knowledge through their own admission flow.

## Specification contents

| Part | File | Conformance |
|---|---|---|
| 0: Preliminaries | [spec/00-preliminaries.md](spec/00-preliminaries.md) | |
| 1: Core data model | [spec/01-data-model.md](spec/01-data-model.md) | L0 |
| 2: Schema (ontology and taxonomy) | [spec/02-schema.md](spec/02-schema.md) | L0 |
| 3: Changeset substrates | [spec/03-substrates.md](spec/03-substrates.md) | L1 |
| 4: Governance | [spec/04-governance.md](spec/04-governance.md) | L2 |
| 5: Provenance and attestation | [spec/05-provenance-attestation.md](spec/05-provenance-attestation.md) | L1/L3 |
| 6: Principals and identity | [spec/06-principals.md](spec/06-principals.md) | L1 |
| 7: Serialization bindings | [spec/07-serializations.md](spec/07-serializations.md) | L0 |
| 8: Indexing contract | [spec/08-indexing.md](spec/08-indexing.md) | L0/L3 |
| 9: Federation | [spec/09-federation.md](spec/09-federation.md) | L1+F |
| 10: Non-goals | [spec/10-non-goals.md](spec/10-non-goals.md) | |
| Appendices A to H | [spec/appendices.md](spec/appendices.md) | |

## Conformance levels at a glance

| Level | Name | An implementation provides |
|---|---|---|
| **L0** | Data model | Core model, schema, and at least one serialization binding |
| **L1** | Verifiable history | Changesets, signatures, state hashing, replayable log |
| **L2** | Governed | Policy evaluation on admission, signed decision records |
| **L3** | Attested | Indexing and/or admission inside attested environments (TEE) |
| **F** | Federated | Graph identity, grants, share bundles, sync, imports. Claimable at L1 or above |

Levels are cumulative, and F composes with any level from L1 up. The
reference MVP targets L2 locally, demonstrates L3 once with a single
attested accept/reject cycle, and demonstrates one federated exchange
between two local graphs.

## The mental model

Allod generalizes the git model:

1. A graph is a content-addressed DAG of signed changes. The current state
   is a deterministic fold of history.
2. Where git versions files, Allod versions typed knowledge: entities,
   relationships, and classifications.
3. The graph declares machine-evaluable rules about who may change what.
   The rules are versioned objects inside the graph.
4. Classification and admission can run inside attested hardware. A third
   party can then verify both what the graph says and that every change
   that produced it followed the declared rules.
5. Graphs federate the way git repositories do: a graph pulls changesets
   from a peer within a granted scope and admits them under its own
   policy. Provenance survives the crossing, so a claim can be traced
   through every graph it passed through.
