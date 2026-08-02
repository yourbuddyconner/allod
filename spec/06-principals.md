# Part 6: Principals and Identity *(L1)*

## 6.1 Principal model

A principal is an identity that can author changesets. Principals are
graph objects, defined as entity types in the core ontology, so identity
changes are governed, provenance-carrying history like everything else.

There are three kinds:

| Kind | Description | Examples |
|---|---|---|
| `user` | A human with root or delegated authority | The graph owner. A reviewer |
| `service` | A deterministic system component | A sync daemon. A CI runner. An admission gate |
| `agent` | An AI system acting under delegation from a user or an org | A personal assistant writing memory. A code-review agent |

The kind has direct policy consequences. Selectors key on it (§4.2
`authors`),
and default postures SHOULD differ by kind: what a `user` may assert
directly, an `agent` may only propose. **An agent acts as an extension
of its delegating user, and all of its authority comes from that
delegation.** Every agent principal
carries a `delegated_by` reference and acts within that delegation's
scope.

## 6.2 Credentials and signature suites

| Field of a principal's key record | Description |
|---|---|
| `key_id` | Hash of the public key |
| `algorithm` | REQUIRED support: Ed25519 and ECDSA P-256. Others by registry |
| `valid_from` / `valid_until` | Key validity window. Evaluated against the DAG position (parent state) of the changeset under verification |
| `status` | `active` \| `rotated` \| `revoked` |

Key rotation and revocation are ordinary governed changesets. Historical
signature verification uses the key state as of the changeset's parent.
Revoking a key today does not invalidate yesterday's legitimately signed
history. It does invalidate trust in anything the key signs after the
revocation is admitted.

## 6.3 The indexer is a principal

**Any process that derives knowledge authors its changes as a principal,
subject to policy, carrying lineage. This includes model-assisted
classification.**

No process writes state without authorship: no importer, no migration
tool, and no indexer bypasses the principal model. When an LLM classifies a document into the graph, the
classification arrives as a changeset, authored by an `agent` principal,
with lineage that records `method: model-assisted` and names the model in
`tool`. At L3 it also carries an attestation envelope.

This rule is what makes the sovereign-memory claim true: "my AI wrote
this into my memory, under my rules, and I can prove it."

## 6.4 Binding profiles

The core identity model is bare keys. Design principle 2 holds: a laptop
and an Ed25519 keypair are a complete deployment. Profiles bind
principals to richer identity systems. Profiles are non-normative.

### 6.4.1 Plain-keypair profile (reference)

Keys are generated and held locally. The root authority is a file. This
is the MVP profile and the minimum every implementation must support.

### 6.4.2 Enclave-custodied profile (e.g. Turnkey-style systems)

This profile serves deployments that want non-custodial key
infrastructure, delegation machinery, and policy-engine enforcement.

- A graph's root authority maps to an isolated identity domain (a
  sub-organization). Its keys live in enclaves, outside any operator's
  reach, so the graph stays sovereign whether hosted or self-hosted.
- `agent` principals map to scoped delegation credentials in the
  session-profile style: named, time-boxed, bound to a key, scoped by a
  policy expression. Delegation-scope enforcement composes with graph
  policy (§4.2 `authors`) and MUST NOT replace it.
- Decision-record and attestation signing can use the identity system's
  enclave-resident signers. The graph owner then gets L3-grade envelopes
  without operating TEE infrastructure.

### 6.4.3 OIDC / federated profile

A principal can also bind to an OIDC subject: issuer plus audience plus
subject. Existing org identity (SSO) can then satisfy `reviewers`
requirements. The OIDC binding is evidence about a principal, and the
principal's Allod key still signs every changeset. Federated identity
adds evidence on top of key-based authorship, which remains the
foundation.
