# Part 6 — Principals & Identity *(L1)*

## 6.1 Principal model

A principal is an identity that can author changesets. Principals are
graph objects, defined as entity types in the core ontology. Identity
changes are therefore governed, provenance-carrying history like
everything else.

There are three kinds:

| Kind | Description | Examples |
|---|---|---|
| `user` | A human with root or delegated authority | The graph owner. A reviewer |
| `service` | A deterministic system component | A sync daemon. A CI runner. An admission gate |
| `agent` | An AI system acting under delegation from a user or an org | A personal assistant writing memory. A code-review agent |

The kind is not cosmetic. Policy selectors key on it (§4.2 `authors`).
Default postures SHOULD differ by kind: what a `user` may assert directly,
an `agent` may only propose. **An agent is an extension of its delegating
user. It is never an independent authority.** Every agent principal
carries a `delegated_by` reference and acts within that delegation's
scope.

## 6.2 Credentials and signature suites

| Field of a principal's key record | Description |
|---|---|
| `key_id` | Hash of the public key |
| `algorithm` | REQUIRED support: Ed25519 and ECDSA P-256. Others by registry |
| `valid_from` / `valid_until` | Key validity window. Evaluated against the DAG position (parent state), not wall clocks |
| `status` | `active` \| `rotated` \| `revoked` |

Key rotation and revocation are ordinary governed changesets. Historical
signature verification uses the key state as of the changeset's parent.
Revoking a key today does not invalidate yesterday's legitimately signed
history. It does invalidate trust in anything the key signs after the
revocation is admitted.

## 6.3 The indexer is a principal

This is the most consequential rule in this Part:

**Any process that derives knowledge authors its changes as a principal,
subject to policy, carrying lineage. This includes model-assisted
classification.**

There is no side door. No "importer" or "indexer" writes state without
authorship. When an LLM classifies a document into the graph, the
classification arrives as a changeset. An `agent` principal authors it.
Its lineage records `method: model-assisted` and names the model in
`tool`. At L3 it carries an attestation envelope.

The sovereign-memory claim is this rule, not a feature built on top of
it: "my AI wrote this into my memory, under my rules, and I can prove
it."

## 6.4 Binding profiles

The core identity model is bare keys. Design principle 2 holds: a laptop
and an Ed25519 keypair are a complete deployment. Profiles bind
principals to richer identity systems. Profiles are non-normative.

### 6.4.1 Plain-keypair profile (reference)

Keys are generated and held locally. The root authority is a file. This
is the MVP profile. It is the floor every implementation must support.

### 6.4.2 Enclave-custodied profile (e.g. Turnkey-style systems)

This profile serves deployments that want non-custodial key
infrastructure, delegation machinery, and policy-engine enforcement.

- A graph's root authority maps to an isolated identity domain (a
  sub-organization). Its keys live in enclaves, outside any operator's
  reach. The graph stays sovereign whether hosted or self-hosted.
- `agent` principals map to scoped delegation credentials in the
  session-profile style: named, time-boxed, bound to a key, scoped by a
  policy expression. Delegation-scope enforcement composes with graph
  policy (§4.2 `authors`). It MUST NOT replace graph policy.
- Decision-record and attestation signing can use the identity system's
  enclave-resident signers. The graph owner then gets L3-grade envelopes
  without operating TEE infrastructure.

### 6.4.3 OIDC / federated profile

A principal can also bind to an OIDC subject: issuer plus audience plus
subject. Existing org identity (SSO) can then satisfy `reviewers`
requirements. The OIDC binding is evidence about a principal. The
principal's Allod key still signs. Federated identity augments key-based
authorship. It never replaces it.
