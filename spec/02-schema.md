# Part 2: Schema (Ontology and Taxonomy) *(L0)*

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
| `parents` | list<term-ref> | A term MAY have multiple parents, so the taxonomy forms a DAG |
| `status` | enum | `active` \| `deprecated` |

Classification semantics: when a subject is classified under a term, policy
treats it as classified under all ancestor terms (§4.1). Descendant terms
are not implied. Deprecated terms stay valid for historical
classifications, but new classifications MUST NOT use them.

## 2.3 Extension and inheritance

Extension is the adaptation path for agents: start from a base ontology
and evolve it locally.

- A derived entity type MAY add attributes. It MAY narrow constraints, for
  example make an optional attribute required or tighten a range. It MUST
  NOT remove inherited attributes or widen inherited constraints. Every
  subtype instance therefore stays a valid instance of the base type.
- A derived ontology imports a base ontology by content hash:
  `imports: [{ ontology: "core", state_hash: "sha256:…" }]`. The hash
  alone identifies the imported ontology. The name exists for
  human-readable projections.
- As a consequence, a consumer that understands only the base ontology can
  still consume a projection of data authored under the derived one, by
  dropping the unknown attributes. Sharing an evolved ontology never
  requires sharing the data classified under it.

## 2.4 Versioning and migration

- Ontology and taxonomy versions are immutable once admitted. To change a
  type, admit version N+1 through a governed changeset.
- Every object records the schema version it was validated against.
  Historical validity is permanent (§1.2).
- A schema version bump MAY ship migration rules. These are deterministic
  rewrites: `rename attribute`, `split type`, `map term`. Implementations
  apply them to produce version-N+1 views of version-N objects. Migration
  rules are advisory for reads and REQUIRED only when an object is next
  mutated. Old data stays readable forever. Writes happen at the current
  version.
- AT Protocol lexicons inform this design: additive change is cheap, a
  breaking change is a new version, and consumers declare the versions
  they accept.

## 2.5 Schema-as-object

The schema lives inside the graph rather than beside it. Ontology types,
taxonomy terms, and governance policies (Part 4) are objects in the graph. Changesets create
and mutate them like any other data, and governance applies to them like
any other mutation.

This design has three intended consequences:

1. **An ontology can be shared on its own.** To export an ontology,
   export a subgraph (Part 7). An agent that evolved a domain ontology can
   hand that world-model to another agent without sharing any of the
   private data classified under it.
2. **Schema changes have provenance.** The graph records who added an
   entity type, when, and from what discussion it derives, using the same
   lineage machinery as any node.
3. **Schema changes are governed.** An ontology bump is a changeset,
   admitted under the current policy (§4.6). A graph can require elevated
   review for schema mutations, and the reference policy in Appendix C
   does.
