# Part 0: Preliminaries

## 0.1 Status of this document

This is draft v0.3 of the Allod specification. Nothing here is
implementation-stable. The specification is versioned. From v1.0 onward,
the plan is to keep the spec's own change history in an Allod graph, which
applies the Part 4 governance model to the spec itself.

## 0.2 Motivation

Knowledge produced by and for AI agents is locked inside vendor silos.
Current systems lack three properties:

1. **Portability.** An agent's accumulated memory cannot move to another
   system intact. Markdown dumps lose structure. API exports lose
   history.
2. **Verifiability.** No mainstream knowledge store can prove to a third
   party how its contents came to be. There is no way to check who wrote
   each claim, under what authority, or from what source.
3. **Governance.** No format expresses rules about change: who may assert
   facts about which subjects, what review a mutation needs, or which
   classifications require elevated authority. Access control lives in the
   database layer today and does not travel with the knowledge.

Allod makes all three properties part of the format itself, so they do
not depend on any host and are not lost when knowledge changes hands.
Exchange between independent graphs
([Part 9](09-federation.md)) relies on this: knowledge that carries its
own provenance can be verified by whoever receives it.

The design generalizes a practice that already works: code review.
Reviewing code is governing a body of knowledge by governing its diffs.
Allod applies the same move to knowledge in general. When every change is
a signed, well-formed diff, then who may change what, where a fact came
from, and whether the rules were followed are all questions answered by
reading the diff log. Design principle 1 follows from this.

## 0.3 Design principles

These principles are normative. When a later section faces a design
dispute, the resolution MUST agree with them, in priority order:

1. **The log is the source of truth.** Files, tables, and wire messages
   are projections of the changeset log. A projection MUST NOT carry
   state that the log cannot reproduce.
2. **Sovereignty.** No capability of this specification may require a
   specific host, vendor, network service, or hardware supplier. An owner
   with only local, open implementations MUST be able to exercise every
   capability. Attestation hardware is optional and confined to
   conformance level L3.
3. **Verifiability before trust.** Every graph state MUST be
   reconstructible and checkable from signed history alone. A verifier
   needs only the log and the public keys.
4. **Minimality.** The core (L0 and L1) stays small. Additional
   capability lives in higher conformance levels and profiles.
5. **Substrate neutrality.** Governance and provenance are defined over an
   abstract changeset interface. They MUST NOT depend on what the diff is
   a diff of. The same rules therefore apply to a knowledge changeset
   and to a git commit.
6. **Receiver-governed exchange.** Federation is pull-based. A graph
   admits foreign knowledge only through its own admission flow, under
   its own policy. Each graph keeps full authority over its own state.

## 0.4 Terminology

Interpret the key words MUST, MUST NOT, REQUIRED, SHALL, SHOULD, SHOULD
NOT, and MAY as described in RFC 2119 and RFC 8174.

- **Graph.** The materialized set {nodes, edges, classifications,
  documents} produced by replaying a changeset log against a schema.
- **Schema.** The pair (ontology, taxonomy) plus attached governance
  policy. The schema is stored in-graph as versioned objects.
- **Ontology.** The typed entity and relationship model: entity types,
  attribute schemas, edge types, constraints.
- **Taxonomy.** The DAG of classification terms.
- **Changeset.** The atomic unit of mutation. A signed set of operations
  with parent pointers into the log's DAG.
- **Substrate.** A concrete implementation of the abstract changeset log
  interface (Part 3). This spec defines two: native and git.
- **Projection.** A serialization of graph state or log segments into a
  concrete format: markdown bundle, Parquet, or wire form.
- **Principal.** An identity that can author changesets: a user, a
  service, or an agent.
- **Indexer.** A principal, usually a service or agent, that derives
  classifications and structure from documents (Part 8).
- **Classification.** The assignment of a node, edge, or document to a
  taxonomy term under a specific ontology version.
- **Decision record.** A signed artifact that records a governance verdict
  over a proposed changeset (Part 4).
- **Attestation envelope.** A signed statement that binds a changeset, the
  policy version it was evaluated under, and the decision. It can carry
  hardware attestation evidence (Part 5).
- **State hash.** The deterministic digest of a materialized graph (§1.7).
- **Root authority.** The principal or principals that hold ultimate
  governance authority over a graph. Declared at genesis (§4.6).
- **Graph ID.** The hash of the genesis changeset. The self-certifying
  identity of a graph (§9.2).
- **Peer.** Another graph known to this one, recorded as a peer record
  with its graph ID, keys, and locator hints (§9.3).
- **Grant.** A signed, governed object that authorizes disclosure of a
  scope to an audience (§9.4).
- **Share bundle.** The federation transfer artifact: a checkpoint
  reference plus disclosed objects or elided changesets with proofs
  (§9.5).
- **Mirror / Import.** The two ways a graph holds foreign knowledge:
  kept verbatim outside the admitted state (mirror), or adopted through
  the graph's own admission flow (import) (§9.6).

## 0.5 Conformance levels

Conformance is cumulative. A conformance level describes the capabilities
of an implementation.

- **L0 (data model).** Implements Parts 1 and 2, and at least one
  serialization binding from Part 7. Sufficient for read/write tooling and
  projections.
- **L1 (verifiable history).** Adds Part 3 with at least one substrate,
  Part 6 signatures, and state hashing. Sufficient to author, exchange,
  and verify logs.
- **L2 (governed).** Adds Part 4: policy evaluation on changeset admission
  and signed decision records. L2 has two strengths, defined in §4.5:
  L2-observed (audit after the fact) and L2-enforced (admission in the
  write path).
- **L3 (attested).** Adds Part 5 attestation. Indexing and/or admission
  run inside an attested environment (TEE). The environment emits
  attestation envelopes that a third party can verify against hardware
  vendor roots.

**F (federated)** is a capability independent of the levels, claimable
at L1 or above: graph identity, peer records, grants, share bundles, and
sync (Part 9). At L2, imports run through admission. At L3, grants can
require attestation.

## 0.6 Prior art and positioning

This section gives a short answer to "why not X" for each entry below.
The RDF mapping is normative in Appendix D.

- **RDF / OWL / SPARQL.** The most complete prior formalization of typed
  knowledge graphs. RDF has no changeset primitive. Named graphs and
  RDF-star do not provide signed, ordered history. RDF has no governance
  model and no attestation mechanism. Three decades of practice show
  that application developers work around RDF rather than with it. Allod
  accepts the lesson: triples are too low-level a form to author
  knowledge in directly. Appendix D preserves compatibility through a
  lossy export mapping.
- **JSON-LD / schema.org.** These provide vocabulary and serialization
  but no history, authority, or governance. An Allod ontology MAY import
  schema.org type vocabularies.
- **Datomic / XTDB and other immutable-log databases.** These prove that
  a log as the source of truth works in production, but they are products
  with internal logs rather than interchange formats. Their logs are not
  portable, signed, or governed.
- **git.** git demonstrates at scale that a content-addressed DAG of
  signed diffs with deterministic state works. Allod adopts git as
  a substrate (§3.3) instead of reinventing it, and git's remote-and-patch
  exchange shapes the sync model of Part 9.
- **LSIF / SCIP.** These formats precompute code intelligence
  (definitions, references, hovers) as data. Allod adopts them as the
  preferred edge source for the code ontology (§8.3) instead of
  reinventing them.
- **W3C Verifiable Credentials / C2PA.** Prior art for attestation
  envelopes. Allod's envelope (§5.2) is shaped so a VC or C2PA manifest
  can carry it.
- **AT Protocol lexicons.** The best current example of schema evolution
  in a federated system. Informs the versioning semantics of §2.4.
- **ActivityPub / Matrix.** Push-based social federation with
  server-mediated trust. Allod's exchange is pull-based and
  receiver-governed (Part 9), closer in shape to git remotes than to
  social broadcast.
- **"Open Knowledge Format" (Google) and Stanford KIF.** Name collisions
  only. Neither shares this design. Allod avoids both names.

## 0.7 Document conventions

Field layouts appear as tables with CDDL-style type notation. Examples use
YAML for readability. The normative encoding is the canonical wire form
(§7.1). Example hashes are truncated (`sha256:ab12…`) for legibility.
