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
   system in a form that system can faithfully consume. Markdown dumps lose
   structure. API exports lose history.
2. **Verifiability.** No mainstream knowledge store can prove to a third
   party how its contents came to be. There is no way to check who wrote
   each claim, under what authority, or from what source.
3. **Governance.** No format expresses rules about change: who may assert
   facts about which subjects, what review a mutation needs, or which
   classifications require elevated authority. Access control lives in the
   database layer today and does not travel with the knowledge.

Allod makes all three properties intrinsic to the format, independent of
the host.

The founding axiom is a generalization from code review: **all governance
is the administration of diffs.** When every change to a body of knowledge
is a signed, well-formed diff, governance, provenance, and audit all reduce
to operations over the diff log. This axiom drives design principle 1.

## 0.3 Design principles

These principles are normative. When a later section faces a design
dispute, the resolution MUST agree with them, in priority order:

1. **Projection, not format.** The changeset log is the sole source of
   truth. Files, tables, and wire messages are projections of the log. A
   projection MUST NOT carry state that the log cannot reproduce.
2. **Sovereignty.** No capability of this specification may require a
   specific host, vendor, network service, or hardware supplier. An owner
   with only local, open implementations MUST be able to exercise every
   capability. Attestation hardware is an optional strengthening, confined
   to conformance level L3.
3. **Verifiability before trust.** Every graph state MUST be
   reconstructible and checkable from signed history alone. A verifier
   needs only the log and the public keys, never the cooperation of the
   graph's host.
4. **Minimality.** The core (L0 and L1) is small. Capability lives in
   conformance levels and profiles, not in the core.
5. **Substrate neutrality.** Governance and provenance are defined over an
   abstract changeset interface. They MUST NOT depend on what the diff is
   a diff of. Knowledge changesets and git commits are peers.

## 0.4 Terminology

Interpret the key words MUST, MUST NOT, REQUIRED, SHALL, SHOULD, SHOULD
NOT, and MAY as described in RFC 2119 and RFC 8174.

- **Graph.** The materialized set {nodes, edges, classifications,
  documents} produced by folding a changeset log against a schema.
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

## 0.5 Conformance levels

Conformance is cumulative. It applies to an implementation, not to a graph.

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

## 0.6 Prior art and positioning

This section gives a short answer to "why not X" for each entry below.
The RDF mapping is normative in Appendix D.

- **RDF / OWL / SPARQL.** The most complete prior formalization of typed
  knowledge graphs. RDF has no changeset primitive. Named graphs and
  RDF-star do not provide signed, ordered history. RDF has no governance
  model and no attestation story. Three decades of practice show that
  agents and application developers route around its ergonomics. Allod
  accepts RDF's lesson that triples are too low-level an authoring
  surface. Appendix D preserves compatibility through a lossy export
  mapping.
- **JSON-LD / schema.org.** Vocabulary and serialization without history,
  authority, or governance. An Allod ontology MAY import schema.org type
  vocabularies.
- **Datomic / XTDB and other immutable-log databases.** Proof that
  log-as-truth works in production. They are products, not interchange
  formats. Their logs are not portable, signed, or governed.
- **git.** Proof at planetary scale that a content-addressed DAG of signed
  diffs with deterministic state is the right shape. Allod adopts git as a
  substrate (§3.3) instead of reinventing it.
- **LSIF / SCIP.** Code intelligence as data: definitions, references, and
  hovers, precomputed. Allod adopts them as the preferred edge source for
  the code ontology (§8.3) instead of reinventing them.
- **W3C Verifiable Credentials / C2PA.** Prior art for attestation
  envelopes. Allod's envelope (§5.2) is shaped so a VC or C2PA manifest
  can carry it.
- **AT Protocol lexicons.** The best current example of schema evolution
  in a federated system. Informs the versioning semantics of §2.4.
- **"Open Knowledge Format" (Google) and Stanford KIF.** Name collisions
  only. Neither shares this design. Allod avoids both names.

## 0.7 Document conventions

Field layouts appear as tables with CDDL-style type notation. Examples use
YAML for readability. The normative encoding is the canonical wire form
(§7.1). Example hashes are truncated (`sha256:ab12…`) for legibility.
