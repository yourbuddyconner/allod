# Part 9: Federation *(L1 + F)*

## 9.1 Model

Allod's core properties, verifiable provenance and governed change, are
what make knowledge safe to exchange. Federation is the layer that puts
them to work between graphs: how graphs identify each other, authorize
disclosure, exchange verifiable history, and absorb each other's
knowledge.

Exchange is pull-based and happens between sovereign graphs:

1. A graph discloses knowledge by producing a **share bundle**: a
   verifiable slice of its state and history, authorized by a signed
   **grant** (§9.4, §9.5).
2. A graph absorbs knowledge by pulling bundles from **peers** and
   deciding under its own policy what to admit (§9.6, design
   principle 6).
3. Provenance crosses the boundary intact. An imported claim references
   the foreign objects, changesets, and documents it derives from by
   content hash, so lineage stays verifiable end to end.

Two federated graphs stay fully sovereign. Each keeps its own root
authority (§4.6), and exchange requires only a byte channel between
them.

## 9.2 Graph identity and references

- The **graph ID** is the hash of the genesis changeset. It is
  self-certifying: any holder of the log can recompute it, and a log
  with a different genesis is a different graph.
- The reference form `allod:<graph-id>[/<logical-id>[@<rev>]]` names a
  graph, an object in it, or a specific revision of that object. It is
  valid wherever an `external-ref` is (§1.3) and follows the
  cross-substrate rules of §3.4: the reference carries content hashes,
  and an unresolvable reference degrades a verification result while
  the claim stands.
- Locators such as URLs are hints stored in peer records. Identity
  always comes from hashes.

## 9.3 Peers

A peer record is a graph object, defined in the core ontology alongside
principals (§6.1):

| Field | Description |
|---|---|
| `graph_id` | The peer's genesis hash |
| `root_keys` | The peer's root authority keys as currently known |
| `locators` | Ordered fetch hints: URLs, mirrors, well-known endpoints |
| `trust_basis` | How the keys were learned: `out-of-band` \| `tofu` \| `attested` |

Creating or updating a peer record is a governed changeset, so a graph's
view of who it exchanges with is itself audited history. After the first
key exchange, root authority rotation in the peer's graph is verifiable
by replaying the peer's own governance chain from genesis (§4.6), so
trust bootstraps once per peer.

## 9.4 Grants

A grant authorizes disclosure. It is a signed graph object:

| Field | Description |
|---|---|
| `audience` | A principal, a graph ID, or `public` |
| `scope` | A selector over the graph, with the same grammar as policy selectors (§4.1): taxonomy regions, entity and edge types, operation kinds |
| `rights` | Any of `state` (current objects), `history` (changesets), `subscribe` (repeated sync) |
| `validity` | Time window and expiry behavior |
| `issuer` | Principal ref plus signature |

Rules:

- Issuing, modifying, and revoking a grant are changesets, and policy
  applies to them like any other mutation. A graph can require owner
  sign-off before a region is shared, and the log then answers who
  shared what, with whom, and under whose approval.
- Revocation is a governed changeset that ends future service under the
  grant. Bytes already transferred stay wherever they went. The grant
  log proves what was authorized, when, and for how long.
- A `public` grant makes its scope readable by anyone who can reach a
  locator. Publishing an ontology (§2.5) is a `public` grant scoped to
  the schema subgraph.

## 9.5 Share bundles

A share bundle is the transfer artifact. It contains:

1. A reference to the grant it was produced under.
2. A signed **checkpoint reference** for the source graph: revision
   hash plus state hash (§3.2.5).
3. For the `state` right: the disclosed objects, each with a Merkle
   path to the state hash (§5.4 subgraph proofs).
4. For the `history` right: the changesets in scope, with out-of-scope
   operations elided (§3.2.6). Signatures and parent links stay
   intact.
5. The ontology subgraph the disclosed objects were authored under,
   referenced by state hash (§2.3). The bundle includes it in full on
   first transfer and by reference afterward.

A receiver verifies a bundle with the peer's keys and the checkpoint:
authorship signatures, elision proofs, and subgraph membership.
References that point outside the disclosed scope resolve as degraded
(§3.4). Elision hides operation contents and reveals tree shape, so a
bundle producer MAY pad operation counts where the shape itself is
sensitive.

A bundle is a tagged wire object (§7.1). There is no other normative
bundle encoding.

## 9.6 Mirrors and imports

A receiver holds foreign knowledge in one of two ways:

- **Mirror.** Store the bundle contents verbatim, keyed by graph ID,
  outside admitted state. A mirror supports reads and cross-graph
  references without admission, because nothing enters the local log.
  Mirrors are the federation analogue of git remote-tracking branches.
- **Import.** Create local objects from foreign ones through an
  ordinary proposal (§4.3). The importing principal authors the
  proposal. Lineage on every imported object records `derived_from`
  the foreign object and changeset by `allod:` reference and
  `derived_by` the importer. The foreign author stays visible in
  lineage, the local author in authorship, and both facts verify
  separately.

Import rules:

- Imported objects MUST validate against a local schema. Either import
  the source ontology, which §2.5 makes an exportable subgraph, and
  extend it locally (§2.3), or map foreign types with migration rules
  (§2.4).
- Policy SHOULD route imports through admission requirements at least
  as strict as local model-assisted writes, because a foreign graph's
  admission standards are its own.
- Re-syncing a subscribed scope updates mirrors directly and MAY
  generate follow-up import proposals for adopted objects. The
  explicit-resolution rule (§3.2.3) applies to adopted objects with
  full force.

## 9.7 Sync

Sync transfers the changesets a receiver lacks, within a grant scope:

1. The requester presents the grant and its known head revisions for
   the scope.
2. The responder returns a bundle containing the in-scope changesets
   that are ancestors of its heads and absent from the requester's
   have-set.
3. The requester verifies the bundle, then mirrors or imports per
   §9.6.

Sync is a request/response exchange over any byte channel: HTTP, a
file handed over, a bundle in object storage. The bundle format and
the have/want negotiation define the protocol, and every step is
idempotent and resumable. The `subscribe` right is a standing
permission to repeat the exchange. Appendix G sketches a minimal HTTP
binding.

## 9.8 Conformance

**F** is a capability marker orthogonal to the levels. An
implementation claims it at L1 or above:

- **L1+F.** Graph IDs, peer records, grants, bundles, sync, mirrors.
  Sufficient for replication and read-side federation.
- **L2+F.** Adds governed imports: adoption runs through admission,
  and grant operations are policy-controlled.
- **L3+F.** Grants can require attestation of either side. Attested
  predicates (§5.4) let a graph answer a peer's question with a proof
  instead of a disclosure.
