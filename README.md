# Allod

**An open format for knowledge graphs that you own, verify, and govern.**

The name comes from allodial title: the legal term for property that the
holder owns absolutely, free of any landlord or superior authority. This
spec applies that goal to knowledge — you own the graph outright, its
history is verifiable, and changes follow rules that you declare.

| | |
|---|---|
| **Status** | Draft v0.3. For discussion. Not implementation-stable. |
| **Author** | Conner Swann |
| **Date** | 2026-08-02 |

## What it is

Allod defines a file format and a set of rules for a knowledge graph — a
collection of typed records (people, notes, contracts, functions) connected
by typed relationships. Three properties define the format:

- **You own it.** The graph lives in files you control. Nothing about it
  depends on a hosting service or a vendor.
- **Anyone can verify it.** The graph is stored as an append-only log of
  signed changes. Anyone with the log and the public keys can rebuild the
  current state and check every step of its history.
- **Changes follow declared rules.** The graph carries its own policy: a
  machine-checkable statement of who may change what, and what approvals
  each kind of change needs.

The core idea borrows from git. A git repository stores commits and builds
the working tree by replaying them. An Allod graph stores **changesets** —
signed batches of changes — and builds the current state by replaying
those. Each changeset is checked against a versioned schema as it is
applied. Everything else you might export (markdown files, Parquet tables,
messages sent over the wire) is a view generated from that state and can be
regenerated at any time. The log is the only source of truth.

Because the rules attach to changes rather than to any particular kind of
content, the same machinery — policy checks, history of who did what, and
proof of what software did it — works on anything with a compatible log.
The spec defines its own native log format and also treats a git repository
as such a log, so the same rules and audit tools apply to a knowledge graph
and to a codebase.

Graphs can also share knowledge with each other (Part 9 of the spec). Each
graph has a permanent identity (the hash of its first changeset). A graph
can give another graph a signed **grant**: permission to read a defined
slice of its data. The receiving graph imports that data through its own
approval rules, and the record of where each fact came from survives the
transfer. A claim can be traced back through every graph it passed through.

## Try it

The reference CLI runs the founding use case — an AI assistant's memory,
governed by its owner — end to end on one machine:

```
cargo run -p allod -- demo /tmp/allod-demo
```

The demo:

1. Creates a graph with a root keypair.
2. Registers an AI agent with limited, delegated authority.
3. Lets the agent write a scratch note, which is admitted immediately.
4. Has the agent propose a preference about the owner. The policy holds
   this change for the owner's approval instead of admitting it.
5. Records the owner's signed approval, which admits the change.
6. Verifies the whole history from the log and public keys alone, at three
   levels: the hashes are intact, the signatures are genuine, and every
   change followed the declared policy.

The individual commands (`init`, `agent-add`, `note`,
`propose-preference`, `approve`, `verify`, `show`) run the same flow by
hand: see [crates/allod](crates/allod/). Two more demos cover the rest of
the spec's acceptance scenario (Appendix A), which also runs as an
integration test:

- `demo-code` imports a git repository as a governed graph of its code,
  then holds a commit for review because it touches code classified as
  security-critical.
- `demo-federation` moves a preference from one graph to another under a
  grant, proves the shared data is genuine, and shows that sharing stops
  after the grant is revoked.

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

## Conformance levels

An implementation does not have to support everything. The spec defines
cumulative levels, and the table above marks which parts each level
requires:

| Level | Name | An implementation provides |
|---|---|---|
| **L0** | Data model | The core model, schemas, and at least one file format |
| **L1** | Verifiable history | Signed changesets, state hashing, and a replayable log |
| **L2** | Governed | Policy checks on every change, with signed approval records |
| **L3** | Attested | Processing inside trusted hardware (a TEE) that can prove which code ran |
| **F** | Federated | Graph identity, grants, and imports between graphs. Claimable at L1 or above |

The reference implementation targets L2 for local use, demonstrates L3
once with a single attested accept/reject cycle, and demonstrates one
exchange between two local graphs.

## Reference ontologies

An **ontology package** defines the record types, relationship types,
classification terms, and change rules for one domain. These packages ship
with the spec as worked examples:

| Package | Path | Contents |
|---|---|---|
| `core` | [ontologies/core/](ontologies/core/) | The minimal shared vocabulary: people, companies, identities, peer graphs, grants |
| `corp` | [ontologies/corp/](ontologies/corp/) | A base company ontology: org structure, external parties, contracts, work artifacts, plus classification terms and a reference policy |
| `code` | [ontologies/code/](ontologies/code/) | Code graphs built from git repositories: files, functions, types, and their relationships |
| `eng` | [ontologies/eng/](ontologies/eng/) | An engineering organization: services, change management, code review, incidents |
| `memory` | [ontologies/memory/](ontologies/memory/) | Personal AI-assistant memory, with owner approval for changes that matter |
| `research` | [ontologies/research/](ontologies/research/) | Claims, the evidence behind them, and a review process for publishing |
| `supply` | [ontologies/supply/](ontologies/supply/) | A supply chain spanning several organizations, with controlled disclosure |
| `grc` | [ontologies/grc/](ontologies/grc/) | Compliance: controls, audit evidence, findings, exceptions |
| `spec` | [ontologies/spec/](ontologies/spec/) | The specification managing its own change history |

Two crates back these packages:

- [allod-core](crates/allod-core/) is the shared library: the spec's
  vocabulary, package and taxonomy registries, reference resolution, the
  attribute type grammar, the canonical byte encoding, and the hashing
  rules.
- [allod-lint](crates/allod-lint/) validates every package against the
  spec's rules and checks that each declared import hash matches the
  actual content of the imported package. Run it with
  `cargo run --release -p allod-lint`.

The spec also ships known-good test values in
[spec/vectors/](spec/vectors/): sample inputs with their expected hashes,
so an independent implementation can confirm it computes the same results.
They are generated by [allod-vectors](crates/allod-vectors/) and kept
reproducible by CI.

One caveat on the packages: the YAML files here are human-readable
snapshots. In a live graph, the same definitions are stored as versioned
objects inside the graph, and imports refer to them by hash.

## The mental model

Allod generalizes the git model:

1. A graph is a content-addressed chain of signed changes. The current
   state is the deterministic result of replaying its history.
2. Where git versions files, Allod versions typed knowledge: records,
   relationships, and classifications.
3. The graph declares machine-checkable rules about who may change what.
   The rules are themselves versioned objects inside the graph.
4. Classification and change approval can run inside trusted hardware. A
   third party can then verify both what the graph says and that every
   change that produced it followed the declared rules.
5. Graphs share the way git repositories do: a graph pulls changesets from
   a peer, within the scope that peer granted, and admits them under its
   own rules. The origin of each fact survives the transfer, so a claim
   can be traced through every graph it passed through.
