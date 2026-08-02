# Part 5: Provenance and Attestation *(L1 lineage / L3 attestation)*

## 5.1 Lineage model

Every object revision carries lineage. Lineage is REQUIRED at L1.

| Field | Description |
|---|---|
| `derived_from` | Document refs and/or object refs this revision derives from |
| `derived_by` | The principal that produced it: a human, a service, or an indexer |
| `method` | `manual` \| `deterministic` \| `model-assisted` |
| `tool` | For non-manual methods: tool identity plus version, e.g. `rust-analyzer@0.4.2110` or a model ID |
| `changeset` | The changeset that introduced this revision. The log implies this. It is restated for projection convenience |

Lineage composes transitively. A claim derived from a summary derived from
a transcript carries the full chain. An implementation MUST be able to
list, from the log alone, every document a given claim rests on.

## 5.2 Attestation envelope

An attestation envelope is a signed statement by an **attester** about an
execution. Lineage records that a principal claims to have done
something. An envelope strengthens that claim into proof that the work
ran inside a specific, measured environment.

| Field | Description |
|---|---|
| `statement` | What is attested: `{ changeset_hash, policy_context_hash?, decision?, indexer_identity?, input_hashes[] }` |
| `attester` | Principal ref of the attesting environment |
| `evidence` | Platform evidence: a TEE quote or measurement chain (e.g. AWS Nitro attestation document, TDX quote), or `none` for a bare signature |
| `evidence_type` | Names the verification procedure for `evidence` |
| `signature` | By the attested environment's key, bound to the measurement in `evidence` |

Design constraints:

- The envelope is agnostic to evidence format. A new TEE platform adds an
  `evidence_type` entry and does not require a spec revision.
- The envelope is shaped so a W3C Verifiable Credential or a C2PA
  manifest can carry it and serve as the outer wrapper.
- An envelope with `evidence: none` is just a signature. It proves only
  that the signer holds the key, without any guarantee about what code
  ran, and verification results MUST distinguish the two.

## 5.3 Verification procedures

A verifier receives a graph, or a subgraph plus checkpoint. It can check
four levels, in increasing strength. Each level needs only the log and
public keys, except level 4, which also needs hardware vendor roots.

1. **Integrity.** Every revision hash and state hash recomputes.
2. **Authorship.** Every changeset signature verifies against its
   principal's registered keys (§6.2), as of that changeset's parent
   state.
3. **Governance** (L2). Every changeset's decision-record set satisfies
   the policy in force at its proposal. This is the audit question of
   §4.5.
4. **Attestation** (L3). Every envelope's evidence chain verifies to a
   trusted measurement root, and the measured code identity is one the
   verifier accepts.

Verification MUST NOT require contact with the graph's host. This
restates design principle 3 in operational form.

## 5.4 Selective disclosure

A holder can prove properties of a graph without revealing its contents.
These primitives are the disclosure layer that federation shares are
built from (§9.5).

- **Subgraph proofs.** The state hash is a Merkle root (§1.7). A holder
  can reveal a subgraph plus Merkle paths, which proves membership in a
  state without revealing siblings.
- **Attested predicates.** At L3, an attested environment can evaluate a
  predicate over a private graph and emit an envelope that attests the
  result. Example: "a node classified `kyc/verified` exists for subject
  S." The envelope reveals that fact without revealing the node
  itself. The predicate's code
  identity is in the measurement, so the verifier knows exactly what
  question was answered.
- **Redacted projections.** A redacted projection (Part 7) carries
  tombstone hashes, so its state stays checkable against the full
  graph's state hash.

## 5.5 Attested execution (L3)

Only two workloads justify TEE execution, and both are deliberately
small:

1. **The attested indexer** (Part 8). Classification runs inside an
   attested environment. This matters most for model-assisted
   classification. The envelope binds the input document hashes, the
   indexer code identity, the model identity, and the output changeset
   hash. Consumers can then trust machine-written knowledge to exactly
   the degree they trust the measured pipeline. The setting where this
   matters most is multi-writer and federated graphs, where writers do
   not trust each other.
2. **The attested admission gate** (§4.5). The L2-enforced write path
   runs attested, so a third party can verify that nothing enters the
   graph except through policy. The statement "our process requires
   review" changes from an organizational claim into a cryptographic
   one, which is the property compliance regimes need.

L3 is an optional strengthening, and nothing outside this section
depends on it (design principle 2). The reference MVP demonstrates it
once but does not require it throughout.
