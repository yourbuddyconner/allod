# Part 2 — Schema: Ontology + Taxonomy *(L0)*

## 2.1 Ontology

An ontology declares the typed shape of a graph.

**Entity types.** Each entity type declares:

| Field | Type | Description |
|---|---|---|
| `name` | string | Namespaced, e.g. `core/Person` |
| `version` | int | Monotonic. Versions are immutable once admitted |
| `extends` | type-ref (optional) | Single inheritance from another entity type (§2.3) |
| `attributes` | map<name, attribute-schema> | Type, required or optional, default, constraints |
| `constraints` | list<constraint> | Cross-attribute invariants, e.g. `end >= start` |

**Edge types.** Each edge type declares a `name`, a `version`, a `domain`
(allowed `from` entity types), a `range` (allowed `to` types), a
cardinality (`one-to-one` | `one-to-many` | `many-to-many`), and attribute
schemas.

**Validation rules.** An ontology MAY declare validation predicates beyond
per-object schema validity. Example: "a `Company` node must gain at least
one `registered-in` edge within the same changeset."

## 2.2 Taxonomy

A taxonomy is a DAG of terms used to classify subjects.

| Field | Type | Description |
|---|---|---|
| `name` | string | Namespaced, e.g. `sensitivity/private` |
| `version` | int | |
| `parents` | list<term-ref> | Multiple parents permitted. The structure is a DAG, not a tree |
| `status` | enum | `active` \| `deprecated` |

Classification semantics: when a subject is classified under a term, policy
treats it as classified under all ancestor terms (§4.1). Descendant terms
are not implied. Deprecated terms stay valid for historical
classifications. New classifications MUST NOT use them.

## 2.3 Extension and inheritance

This is the adaptation path for agents: start from a base ontology, evolve
locally.

- A derived entity type MAY add attributes. It MAY narrow constraints, for
  example make an optional attribute required or tighten a range. It MUST
  NOT remove inherited attributes. It MUST NOT widen inherited
  constraints. Every subtype instance stays a valid instance of the base
  type.
- A derived ontology imports a base ontology by content hash, not by name:
  `imports: [{ ontology: "core", state_hash: "sha256:…" }]`. Name
  resolution without a hash is a projection convenience. It is never an
  identity.
- Consequence: a consumer that understands the base ontology can consume a
  projection of data authored under the derived one. It drops unknown
  attributes. Sharing an evolved ontology never requires sharing the data
  classified under it.

## 2.4 Versioning and migration

- Ontology and taxonomy versions are immutable once admitted. To change a
  type, admit version N+1 through a governed changeset.
- Every object records the schema version it was validated against.
  Historical validity is permanent (§1.2).
- A schema version bump MAY ship migration rules. These are deterministic
  rewrites: `rename attribute`, `split type`, `map term`. Implementations
  apply them to produce version-N+1 views of version-N objects. Migration
  rules are advisory for reads. They are REQUIRED only when an object is
  next mutated. You can read old data forever. You write at the current
  version.
- AT Protocol lexicons inform this design. Additive change is cheap. A
  breaking change is a new version. Consumers declare the versions they
  accept.

## 2.5 Schema-as-object

The schema is not a sidecar file. Ontology types, taxonomy terms, and
governance policies (Part 4) are objects in the graph. Changesets create
and mutate them like any other data. Governance applies to them like any
other mutation.

Three consequences, all intended:

1. **Shareable ontologies come free.** To export an ontology, export a
   subgraph (Part 7). An agent that evolved a domain ontology can hand
   that world-model to another agent without sharing any private data
   classified under it.
2. **Schema changes have provenance.** Who added this entity type, when,
   and derived from what discussion. The lineage machinery is the same as
   for any node.
3. **Schema changes are governed.** An ontology bump is a changeset,
   admitted under the current policy (§4.6). A graph can require elevated
   review for schema mutations. The reference policy in Appendix C does.
