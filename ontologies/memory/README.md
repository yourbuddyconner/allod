# memory: personal agent memory

The founding use case (§0.2): an assistant that accumulates knowledge
about its owner's life, under the owner's rules, in a format the owner
can take anywhere. The sovereign-memory claim from §6.3 is this
package in operation: "my AI wrote this into my memory, under my
rules, and I can prove it."

## Contents

| File | Contents |
|---|---|
| [ontology.yaml](ontology.yaml) | Notes, preferences, routines, commitments, interests |
| [taxonomy.yaml](taxonomy.yaml) | Life regions, sensitivity, workspace scratch and curated |
| [policy.yaml](policy.yaml) | Agent proposals, owner-approved preferences, private regions |
| [examples/jarvis.yaml](examples/jarvis.yaml) | An agent principal, a free scratch note, and a governed preference |

## Why governance matters in a single-player graph

The agent writes constantly, and the owner cannot review everything.
The policy splits the world in three:

1. **Scratch is free.** Notes in `workspace/scratch` admit without
   review, so the agent thinks at full speed.
2. **Preferences are owner business.** A `Preference` node states what
   the owner wants, and the rule `preferences-are-owner-business`
   means the agent can propose one and only the owner can make it
   true. An assistant that decides your preferences for you has
   crossed a line this policy makes structural.
3. **Private regions are guarded.** `life/health` and `life/finances`
   carry `sensitivity/private` as a parent, so one classification puts
   a record behind owner sign-off.

## Portability is the product

The markdown bundle (§7.2) of this graph is a directory of notes with
front matter, which is the format agent memory systems already read.
The difference is what travels with it: switch assistants and the new
agent receives typed history with provenance instead of a text dump,
and every preference in it can prove who approved it. Exporting the
ontology alone (§2.5) shares the shape of a life without one private
fact.

## Status

Draft, tracks spec v0.3. Import hashes are placeholders until the
reference implementation generates real state hashes (Appendix H).
