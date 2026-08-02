# spec: the specification governing itself

§0.1 promises that from v1.0 onward, the spec's own change history
lives in an Allod graph. This package is the ontology that graph will
use: parts, sections, requirements, the design decisions behind them,
and the test vectors that verify them.

## Contents

| File | Contents |
|---|---|
| [ontology.yaml](ontology.yaml) | Parts, sections, requirements, design decisions, issues, test vectors |
| [taxonomy.yaml](taxonomy.yaml) | Normative and informative regions, stability |
| [policy.yaml](policy.yaml) | Elevated review for normative changes |
| [examples/elision-decision.yaml](examples/elision-decision.yaml) | A real design decision from this spec's history, as graph data |

## Why dogfood

Every claim the spec makes about governed knowledge gets tested
against the most demanding editor available: the spec's own authors.
A normative change is a mutation in the `spec/normative` region, which
demands owner review and a cooling-off window, which produces a
decision record, which any implementer can verify. "Why does §3.2.6
exist" stops being an archaeology question and becomes a `resolved_by`
edge to a design decision with its rationale attached.

The `verifies` edge from test vectors to requirements is the quiet
payoff: when Appendix H lands, conformance coverage is a graph query,
and a requirement with no inbound `verifies` edge is a gap you can
list mechanically.

## Status

Draft, tracks spec v0.3. Import hashes are placeholders until the
reference implementation generates real state hashes (Appendix H).
