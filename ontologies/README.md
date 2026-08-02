# Reference ontologies

These packages are projections (§2.5). In a live graph, every entity
type, taxonomy term, and policy rule here is a versioned object, created
by changesets and governed like any other mutation. The YAML in this
directory is the human-readable form of those objects, in the notation
the spec uses for examples.

| Package | Path | Contents |
|---|---|---|
| `core` | [core/](core/) | The minimal shared vocabulary: Person, Company, principals (§6.1), peers (§9.3), grants (§9.4) |
| `corp` | [corp/](corp/) | The base company ontology: org structure, external parties, contracts, work artifacts, plus a taxonomy and a reference policy |
| `code` | [code/](code/) | The derived code-graph ontology indexers emit into (§8.3): repositories, files, functions, types, and their relationships |
| `eng` | [eng/](eng/) | The engineering organization: services, releases, change management, code review regions, incidents, postmortems |
| `memory` | [memory/](memory/) | Personal agent memory: notes, preferences, commitments, with owner-governed promotion |
| `research` | [research/](research/) | Claims and evidence: sources, quotations, citation chains, review ladder |
| `supply` | [supply/](supply/) | Cross-organization supply chain: parts, batches, certifications, disclosure regions for federation |
| `grc` | [grc/](grc/) | Controls and audit evidence: frameworks, attested collection, findings, exceptions |
| `spec` | [spec/](spec/) | The specification governing itself: parts, requirements, design decisions, test vectors |

## Imports bind by hash

A graph imports a package by state hash, and the name exists for
human-readable projections (§2.3):

```yaml
imports:
  - { ontology: corp, state_hash: "sha256:…" }
```

The hashes in these files are placeholders. The reference
implementation generates the real state hashes, and they land with the
Appendix H test vectors.

## Extension is the intended use

A base package covers the common shape. A company or an agent extends
it with local types and regions (§2.3), and a consumer that understands
only the base still reads projections of the extended data by dropping
unknown attributes. Share the base, extend privately, and exchange
either one on its own.
