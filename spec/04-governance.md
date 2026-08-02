# Part 4 — Governance *(L2)*

## 4.1 Policy model

A **policy** is a schema-adjacent object, stored in-graph (§2.5). It
contains ordered **rules**. Each rule has a *selector* (which changes it
applies to) and *requirements* (what admission demands).

Selectors key on:

- **Taxonomy region** (native substrate). The rule applies to operations
  whose subject is classified under a term or its descendants. This is
  the core mechanism: *classification implies change-policy*. Example:
  anything under `sensitivity/private` requires owner sign-off.
- **Entity or edge type.** Example: all mutations of `core/Policy`
  objects.
- **Path or branch pattern** (git substrate). Repo, path glob, target
  ref.
- **Operation kind.** Example: any `redact-document`, any schema
  mutation.

Selectors compose with AND and OR. When multiple rules match, ALL of
their requirements apply. Requirements are conjunctive. Adding a rule can
only tighten policy. It can never loosen policy.

When no rule matches, the graph's declared **default posture** applies:
`open` (any authenticated principal) or `restricted` (root authority
only). A graph MUST declare its default posture at genesis.

## 4.2 Rule requirements

Implementations MUST evaluate this requirement vocabulary:

| Requirement | Meaning |
|---|---|
| `authors` | Allowed principal kinds, identities, or delegation scopes (§6.1) |
| `reviewers` | Principals or roles whose signed decision records are required. Supports `quorum: n` |
| `review_window` | Minimum and maximum time a proposal stays open. Expiry behavior |
| `schema_valid` | The result must validate. L1 implies this. Policy restates it to strengthen it, e.g. "no deprecated terms" |
| `classification_required` | New subjects of type T must arrive classified under taxonomy X |
| `attestation_required` | The changeset must carry an attestation envelope (§5.2) from a named attester class. Example: "model-assisted classifications only from attested indexers" |
| `substrate_checks` | Substrate-specific predicates. Git examples: CI status attestations present, linear history, signed-off-by |

Requirements reference principals by role, e.g. `role:owner` or
`role:steward`. Roles resolve through the graph's principal objects.
Policy then survives personnel change without edits.

## 4.3 Admission flow and decision records

Admission is a four-step protocol: **propose, evaluate, decide, apply.**

1. **Propose.** A principal publishes a changeset as a *proposal*, with
   intent metadata. A proposal is not yet part of any admitted state.
2. **Evaluate.** An evaluator resolves the policy version in force. The
   normative choice is the policy at the proposal's parent revision. The
   evaluator computes the matching rules and produces a requirement
   checklist. Evaluation is deterministic. The same proposal against the
   same policy state yields the same checklist.
3. **Decide.** Required reviewers issue **decision records**:

   | Field | Description |
   |---|---|
   | `subject` | The proposed changeset's hash |
   | `policy_context` | Hash of the policy version evaluated against |
   | `verdict` | `approve` \| `reject` \| `abstain` |
   | `deciders` | Principal refs plus signatures |
   | `basis` | Optional reference to a review artifact (§4.4) |
   | `timestamp` | |

   A decision record is a document in the graph. It is signed and
   portable. Anyone can verify it without the host. Code-forge reviews
   lack this property. That lack is the reason this Part exists.
4. **Apply.** When the checklist is satisfied, the changeset enters the
   log (native substrate), or the ref advances (git substrate, enforced
   deployments). The admitting event records the checklist and the
   decision records that satisfied it. Every admitted changeset carries
   its own justification.

## 4.4 Review artifacts

A **review artifact** is a document type for structured review. It holds
prose sections anchored to changeset regions (hunks, objects, subgraphs),
reviewer threads, and a verdict state. The verdict state feeds a decision
record's `basis` field. The format defines the artifact, not a UI.
Tooling that structures diffs for human review SHOULD serialize its
packets as review artifacts. Review content then becomes portable and
provenance-carrying, not just verdicts.

## 4.5 Enforcement strengths

L2 has two normatively distinct strengths:

- **L2-observed.** Decision records are recorded. Anyone can audit any
  state after the fact. The audit question is: does every changeset
  reachable from this revision have a satisfying decision-record set,
  under the policy in force at its proposal? The log alone decides this
  question. L2-observed needs no write-path control. It works over hosted
  git remotes today.
- **L2-enforced.** Admission runs in the write path. An unadmitted
  changeset never becomes state. Native substrates get this structurally.
  Git substrates get it only where the deployment controls the remote:
  a pre-receive hook, a merge queue, or a gate service. An **attested
  gate** is the gate service running at L3 (§5.5). With it, a third
  party can verify that the write path was enforced, not merely
  promised.

An implementation MUST NOT claim L2-enforced for a deployment that only
observes. Observation detects violations. Enforcement prevents them. To
conflate the two is the most likely way to oversell this spec.

## 4.6 Policy bootstrap and root authority

- Every graph declares a **root authority** at genesis: one or more
  principal keys, with a threshold such as 2-of-3. The genesis changeset
  is self-admitted by root signature. It is the one changeset exempt from
  policy, because it creates the policy.
- A policy change is a changeset, evaluated under the **previous** policy
  version. No change may authorize itself. The chain of authority from
  genesis to any current rule is therefore fully verifiable.
- Authority transfer and key rotation are policy changes like any other.
  The outgoing authority signs the changeset that installs the new keys.
  When root keys are lost and policy declares no recovery path, the graph
  is unrecoverable. This is by design. Sovereignty includes the
  sovereign's ability to lose the keys. Graphs SHOULD declare recovery
  rules in genesis policy: social recovery, escrowed shards, or similar.

## 4.7 Non-normative mappings

- **Scoped delegation systems.** Session-profile-style grants are named,
  time-boxed, policy-scoped, key-bound credentials. They map onto
  `authors` requirements. A delegated principal's changesets are
  admissible only within the grant's scope. See §6.4.
- **Policy-engine backends.** The requirement vocabulary is small enough
  to compile to existing engines: OPA/Rego, Cedar, or Ump-style
  expression evaluators. The spec defines semantics, not an engine.
- **CODEOWNERS and branch protection.** These express `reviewers`
  requirements with path selectors. Their verdicts are host-locked and
  unsigned. The git binding's value is exact: the same rules, with
  portable signed outcomes.
