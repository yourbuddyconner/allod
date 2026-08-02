# eng: the engineering organization ontology

The human layer of an engineering org, built on `corp` for structure
and `code` for the derived code graph. It covers services and their
ownership, releases and deployments, change management, incidents and
postmortems, and the regions and rules that make code review and
change approval governed history.

## Contents

| File | Contents |
|---|---|
| [ontology.yaml](ontology.yaml) | Services, environments, releases, deployments, change requests, design docs, incidents, postmortems, SLOs |
| [taxonomy.yaml](taxonomy.yaml) | Regions: change, security, reliability, ops |
| [policy.yaml](policy.yaml) | Code review on git, security regions, change windows, postmortem review |
| [examples/payments-change.yaml](examples/payments-change.yaml) | A worked high-risk change: proposal, dual review, deployment, incident, postmortem |

## How change management works

A change request is a node, and its risk classification routes it. Tag
a `ChangeRequest` under `change/high-risk` and the policy demands an
eng-lead decision record plus a 24 hour cooling-off window. Tag it
`change/emergency` and it needs an SRE and a 4 hour clock instead.
The two regions sit side by side in the taxonomy on purpose: ancestor
requirements only tighten (§4.1), so nesting emergency beneath
high-risk would force the cooling-off onto emergencies.

Validation rules close the loop at the data level: a high-risk change
must carry a rollback plan, and a production deployment must arrive in
the same changeset as a `fulfills` edge to an approved change request.
The approval itself is a decision record (§4.3), so "was this deploy
authorized, by whom, under which policy version" is a log query with a
verifiable answer.

## How code review works

Code review runs on the git substrate (§3.3), and this package
supplies the rule shapes:

1. `main-requires-review` keys on repo and ref patterns, the same
   shape as CODEOWNERS and branch protection, with signed portable
   verdicts instead of host-locked settings.
2. The derived code graph makes review risk-aware. Classify a
   function node under `security/critical`, and the
   `security-critical-code` region rule pulls a security reviewer into
   any change whose semantic diff touches it (§8.3). Appendix F walks
   the full flow: propose, derive, evaluate, review artifact, decision
   records, attested gate.
3. Ownership is graph data. `owns` edges from org units to services
   and repositories, and `on_call_for` edges with validity dates,
   replace the org chart lookups that review tooling usually hides in
   config files.

## The learning loop

Incidents point at what caused them (`caused_by` a deployment or a
change request) and what they hurt (`impacts` a service). A postmortem
is a document-anchored node whose region requires eng-lead review, and
its action items are assigned, dated objects. Six months later, "which
tier-0 services had sev1 incidents caused by emergency changes, and
did the action items close" is a query over governed history instead
of an archaeology project.

## Status

Draft, tracks spec v0.3. Import hashes are placeholders until the
reference implementation generates real state hashes (Appendix H).
