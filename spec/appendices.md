# Appendices

## Appendix A: MVP acceptance test (normative for the reference implementation)

The reference implementation passes when this scenario runs end to end on
one machine, with the plain-keypair profile:

1. **Genesis.** Create a graph with a 1-of-1 root authority, the core
   ontology, the code ontology, and the Appendix C policy. Set the
   default posture to `restricted`.
2. **Repo import.** Point the indexer at a small git repository, 50 files
   or fewer. It emits commit-aligned derived changesets under the code
   ontology, using SCIP or tree-sitter extraction. Policy admits all of
   them under the deterministic-indexer rule.
3. **Semantic diff.** Materialize the code graph at two commits. Produce
   the graph diff: changed functions plus the inbound-call blast radius.
   Render it as a review artifact.
4. **Governed admission, accept path.** A `user` principal proposes a
   manual change: classify a module as security-critical. Policy requires
   owner sign-off. The owner issues a decision record. The changeset is
   admitted. Verify the decision record with only the log and public
   keys.
5. **Governed admission, reject path.** An `agent` principal proposes a
   model-assisted classification into `sensitivity/private`. Policy
   requires an attestation. The proposal lacks one. Admission fails. The
   proposal and its rejection stay auditable.
6. **Projections.** Export a markdown bundle. Re-ingest the
   unmodified bundle. The state hash matches (§7.4). Edit one file by
   hand. Re-ingest. Confirm the edit arrives as a proposal that awaits
   admission. (Parquet is an optional binding per §7.3, exercised
   only by implementations that claim it.)
7. **Replay.** Cold-start from the log on a second machine. The state
   hash matches. Repeat from a checkpoint. It matches.
8. **L3, once.** Run the indexer or the admission gate in an attested
   environment. Emit one attestation envelope. Verify its evidence
   chain. A simulated-measurement mode is acceptable for local
   development, but the verification code path must be real.
9. **Federated exchange.** On the second machine, create a second graph
   with its own root authority. On the first, issue a grant scoped to
   one taxonomy region to the second graph's ID and produce a share
   bundle. Transfer it by file copy. On the second graph, verify the
   bundle against the first graph's keys, import one object as a
   proposal, and admit it under local policy. Confirm the imported
   object's lineage traces to the first graph's changeset and document
   hashes. Revoke the grant and confirm the next sync request is
   refused.

Pass means all nine steps succeed with no hosted services. The optional
TEE for step 8 is the only exception.

## Appendix B: Worked example of a core ontology extract plus an agent's extension

Base ontology, extract:

```yaml
ontology: core
version: 1
entity_types:
  Person:
    attributes:
      name:        { type: string, required: true }
      emails:      { type: list<string> }
      affiliation: { type: node-ref, target: Company }
  Company:
    attributes:
      name:    { type: string, required: true }
      domains: { type: list<string> }
edge_types:
  employs:  { domain: Company, range: Person, cardinality: one-to-many }
  knows:    { domain: Person,  range: Person, attributes: { since: { type: date } } }
```

An agent's derived ontology after months of use (§2.3):

```yaml
ontology: conner-workspace
version: 7
imports:
  - { ontology: core, state_hash: "sha256:9f31…" }
entity_types:
  Colleague:
    extends: core/Person
    attributes:
      slack_id:           { type: string }
      one_on_one_cadence: { type: duration }
      routing_notes:      { type: string }   # long text; projects to the markdown body
  Vendor:
    extends: core/Company
    attributes:
      contract_renewal: { type: date }
edge_types:
  escalates_to: { domain: core/Person, range: core/Person, cardinality: many-to-many }
```

Exporting `conner-workspace@7` shares the shape of months of learned
structure (cadences, routing, escalation topology) without exporting any
classified facts. The receiving agent gets the learned structure with
none of the private data recorded under it. This separation is the
payoff of storing the schema in the graph (§2.5).

## Appendix C: Worked example of a governance policy

```yaml
policy: conner-workspace-policy
version: 3
default_posture: restricted
roles:
  owner:   [ "principal:conner" ]
  steward: [ "principal:conner", "principal:jarvis" ]
rules:
  - name: scratch-is-free                        # positive authorization (§4.1)
    select:
      all:
        - { author_kind: agent }
        - { region: "workspace/scratch" }
    require: { schema_valid: true }
  - name: agent-writes-are-proposals
    select:
      all:
        - { author_kind: agent }
        - { not: { region: "workspace/scratch" } }  # scratch is the carve-out
    require:
      reviewers: { role: owner, quorum: 1 }
  - name: deterministic-indexer-writes
    select: { author_kind: service, basis: deterministic }
    require: { schema_valid: true }
  - name: private-region-elevated
    select: { region: "sensitivity/private" }
    require:
      reviewers: { role: owner, quorum: 1 }
      review_window: { max: "72h", on_expiry: reject }
  - name: model-assisted-needs-attestation
    select:
      all:
        - { basis: model-assisted }
        - { not: { region: "workspace/scratch" } }  # carve-outs repeat per rule (§4.1)
    require:
      attestation_required: { attester_class: attested-indexer }
  - name: schema-changes-are-serious
    select: { operation: [define-type, set-policy, deprecate-term] }
    require:
      reviewers: { role: owner, quorum: 1 }
      review_window: { min: "24h" }              # forced cooling-off on rule changes
  - name: git-main-requires-review
    select: { substrate: git, repo: "github.com/me/*", target_ref: "refs/heads/main" }
    require:
      reviewers: { role: steward, quorum: 1 }
      substrate_checks: [ ci-attestation-present ]
```

Scratch takes three rules to be truly free, and each does a different
job. The first *authorizes* scratch writes: under the `restricted`
posture a matching rule is what admits non-root writes (§4.1), so a
carve-out alone would leave scratch writable by root only. The
carve-outs in the following rules then *exempt* scratch from their
requirements, repeated per rule because carve-outs do not compose
across rules: a `not` clause exempts a change from its own rule only.
The final rule governs a git substrate with the same vocabulary as
the knowledge rules.

## Appendix D: RDF / JSON-LD mapping (export, lossy)

| Allod | RDF |
|---|---|
| Node | Resource. IRI minted from graph IRI plus logical ID |
| Attribute | Datatype property assertion |
| Edge | Object property assertion. Edge attributes via RDF-star or reification |
| Classification | `rdf:type` plus a SKOS concept assertion. Taxonomy terms map to `skos:Concept`. The DAG maps to `skos:broader` |
| Ontology | OWL class and property declarations. Constraints OWL cannot express degrade to annotations |
| Changeset / decision record / envelope | **No faithful mapping.** Exportable as named-graph annotations. Signatures and DAG order do not survive |

The mapping is export-only. Round-tripping RDF into Allod is out of
scope. The history, governance, and attestation layers have no
RDF-native home, which is the gap Allod exists to fill.

## Appendix E: Threat model (summary)

| # | Threat | Mitigation |
|---|---|---|
| T1 | History rewrite by the host | Content-addressed DAG plus signatures. A fork with different history changes every downstream hash (§1.7, Part 3) |
| T2 | Unauthorized mutation | L2 admission. L2-enforced write path. §4.5 forbids claiming enforcement while only observing |
| T3 | Malicious or compromised indexer | The indexer is a principal (§6.3): scoped, policy-gated, lineage-carrying. L3 measurement pins the code identity |
| T4 | Model-assisted fabrication | `basis` is first-class. Policy routes model output through stricter admission (§8.2, Appendix C rule 4) |
| T5 | Stolen agent credentials | The agent kind plus delegation scope bounds the blast radius (§6.1). Revocation is a governed changeset. Historical validity is preserved (§6.2) |
| T6 | Projection tampering: an edited bundle passed off as state | A modified projection re-enters as a proposal that must pass admission (§7.2). The state-hash manifest detects divergence |
| T7 | Replay or equivocation across copies | The state hash decides "same graph." Checkpoints are signed, and anchors make equivocation provable from two witnesses (§3.2.5) |
| T8 | Erasure-law conflict with an append-only log | Redaction reaches document bytes, operation content, and intent text while every hash still verifies (§3.2.2, §3.2.6, §8.4). Structural facts survive it: edge endpoints and types are envelope (§1.8), so the existence of a relationship cannot be redacted. Legal analysis is open. The spec flags it rather than hiding it |
| T9 | Root key loss | Unrecoverable when policy declares no recovery path. This is by design. Genesis SHOULD declare recovery rules (§4.6) |
| T10 | TEE compromise or attestation forgery | The envelope pins the evidence type and vendor chain. A failed L3 claim degrades to L2, and verification reports the downgrade (§5.2 requires distinguishing `evidence: none`) |
| T11 | Forged or tampered peer history | Graph IDs are self-certifying (§9.2). Every foreign changeset verifies against the peer's key history, replayed from its genesis (§4.6) |
| T12 | Disclosure beyond the granted scope, or inference from share shape | Elision discloses only chosen operations, with membership proofs (§3.2.6). Merkle shape can leak operation counts, and producers MAY pad (§9.5). Grants are governed and audited (§9.4) |
| T13 | A receiver retains or redistributes shared knowledge after revocation | Revocation ends future service (§9.4). Bytes already transferred stay transferred. The grant log proves what was authorized, when, and for how long |
| T14 | Insider selector evasion: an authorized principal asserts fields (`basis`, lineage, omitted classifications) that route its changes to weaker rules | §4.8 trust classes: audits report rule matches on asserted inputs as degraded. Relaxing rules SHOULD key on log-derived inputs only. `classification_required` (§4.2) closes the unclassified gap. Attestation upgrades asserted inputs (L3) |

## Appendix F: Worked example of a governed git code review, end to end

Cast: the repo `github.com/me/hermes`, the steward role from Appendix C,
an agent reviewer, and an attested gate on `main`.

1. **Propose.** A developer pushes the branch `feat/spend-button`. The
   commit is a changeset (§3.3). It becomes a proposal targeting
   `refs/heads/main`.
2. **Derive.** The indexer updates the derived code graph for the branch
   head (§8.3). The semantic diff against `main` shows: 2 functions
   changed, 7 inbound call sites, and one touched function classified
   `security/spend-path`.
3. **Evaluate.** Policy resolution matches Appendix C rule 6 plus a
   `security/spend-path` region rule — the commit touches the file
   `authorize_spend` derives from, which is the deterministic,
   file-granular reach rule of §8.3. The checklist requires steward
   review, a CI attestation, and owner sign-off from the region rule.
4. **Review.** The agent reviewer walks the full graph: inbound
   callers, and both versions of each function body through `git:`
   refs. It writes a review artifact (§4.4). The artifact's
   sections anchor to the hunks and to the affected subgraph. Its
   verdict is approve-with-comments.
5. **Decide.** The steward and the owner issue decision records that
   reference the artifact. Both records are signed graph documents:
   portable evidence, independent of the forge.
6. **Admit.** The attested gate (§5.5) checks the checklist, advances
   `main`, and emits an attestation envelope. The envelope binds the
   commit SHA, the policy version, and the decision set.
7. **Audit, later.** A third party verifies, from the log, the keys, and
   vendor roots alone, that nothing on `main` ever landed outside
   policy, with no forge cooperation required. No code forge can make
   that guarantee today.

## Appendix G: HTTP sync binding (non-normative)

A minimal binding for §9.7 over HTTPS:

- `GET /.well-known/allod` returns the graph ID and the sync endpoint.
- `POST /allod/sync` with body `{ grant: <ref>, scope_heads: [<hash>…] }`
  returns a share bundle in wire form (§7.1). The responder returns 403
  when no valid grant covers the request and 410 when the grant is
  revoked or expired.
- Responses are cacheable by bundle hash. A static file server hosting
  pre-built bundles for `public` grants is a conforming responder.

The binding adds no state beyond the graph itself. Any transport that
moves the same bytes conforms equally well.

## Appendix H: Test vectors (normative)

v1.0 MUST ship canonical test vectors: a small log in wire form, the
hash of every changeset, the state hash at every revision, one elided
changeset with its membership proofs, one redaction, signature
vectors, and one passing plus one failing governance audit. Two
implementations agree when they reproduce these bytes and hashes
exactly.

The hashing layer's vectors live in [vectors/](vectors/), generated
by `allod-vectors` and held reproducible by CI: canonical preimages,
changeset and revision hashes, operation Merkle roots, state hashes
with a tombstone, the elision proof, the intent redaction, and the
package content hashes that every import declaration binds to
(verified by `allod-lint`). Signature vectors land with Part 6 of the
reference implementation, and the governance audit pair lands with
Part 4.
