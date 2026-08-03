# Reference ontologies

An **ontology package** defines the vocabulary for one domain: the record
types a graph can hold, the relationship types that connect them, a
**taxonomy** (a tree of classification terms, like tags with hierarchy),
and the policy rules that govern changes.

The YAML files in this directory are human-readable snapshots. In a live
graph, every type, term, and rule here is a versioned object inside the
graph, created and changed through the same governed process as any other
data. Spec §2.5 defines the relationship between the two forms.

| Package | Path | Contents |
|---|---|---|
| `core` | [core/](core/) | The minimal shared vocabulary: people, companies, identities that can sign changes, peer graphs, and grants |
| `corp` | [corp/](corp/) | A base company ontology: org structure, external parties, contracts, work artifacts, plus a taxonomy and a reference policy |
| `code` | [code/](code/) | Code graphs built from git repositories: files, functions, types, and their relationships |
| `eng` | [eng/](eng/) | An engineering organization: services, releases, change management, code review, incidents, postmortems |
| `memory` | [memory/](memory/) | Personal AI-assistant memory: notes, preferences, commitments, with owner approval for changes that matter |
| `research` | [research/](research/) | Claims and evidence: sources, quotations, citation chains, and a staged review process |
| `supply` | [supply/](supply/) | A supply chain spanning several organizations: parts, batches, certifications, and controlled disclosure |
| `grc` | [grc/](grc/) | Compliance: control frameworks, audit evidence, findings, exceptions |
| `spec` | [spec/](spec/) | The specification managing its own change history: parts, requirements, design decisions, test vectors |

## Imports bind by hash

A package can build on another package. The import names the package for
readability, but the binding is the hash — a fingerprint of the imported
package's exact content:

```yaml
imports:
  - { ontology: corp, state_hash: "sha256:…" }
```

The hashes in these files are real. Each one is computed over the imported
package's content using the spec's canonical byte encoding (§7.1), by
running `allod-vectors hashes`, and `allod-lint` re-verifies every one on
each run. One caveat: the spec ultimately wants imports to bind to the
hash of the definitions as stored inside a live graph, not to the YAML
snapshot. Until that ships, the snapshot content hash is what imports bind
to, and the switch is a planned pre-v1 breaking change.

## Extension is the intended use

A base package covers the common shape of a domain. A company or an agent
extends it with its own types and classification terms, without modifying
the base. A consumer that understands only the base can still read the
extended data — it simply drops the attributes it does not recognize
(§2.3). This lets you share the base vocabulary publicly, extend it
privately, and exchange either one on its own.
