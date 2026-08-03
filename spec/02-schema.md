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
cardinality (`one-to-one` | `one-to-many` | `many-to-one` |
`many-to-many`), and attribute schemas. Direction is semantic (§1.4), so
`one-to-many` and `many-to-one` are distinct declarations.

**Structs.** An ontology MAY declare named structs: attribute schemas
without identity or lifecycle. A struct name is usable as an attribute
type, including inside `list<>` and `map<>` (§1.3). Struct values
validate structurally and hash as part of their containing object.
Structs keep record-shaped values typed — the key records of §6.2 are
the motivating case — without minting objects for them. Struct names
are global across a graph's installed ontologies, and a graph MUST NOT
admit two structs with the same name.

**Validation rules.** An ontology MAY declare validation rules beyond
per-object schema validity. Rules are structured, not prose, because
fold-time rejection (§3.2.4) depends on them and two implementations
MUST agree on their meaning:

```
rule       := { name, on, require }
on         := { type: entity-type-ref,
                operation?: [create | update],   # default: both
                where?: condition }
condition  := attr-cond | edge-cond
            | { all: [condition…] } | { any: [condition…] }
            | { not: condition }
attr-cond  := { attr: name, equals?: value, in?: [value…],
                present?: bool }
edge-cond  := { edge: { type: edge-type-ref,
                        direction: in | out,
                        min?: int,                # default 1
                        within?: changeset | state,  # default state
                        target_where?: condition } }
require    := condition
```

A rule triggers when a changeset applies a matching operation to an
object of `on.type` or a subtype and `on.where` holds. `where` and
`within: state` conditions evaluate against the state after the
changeset applies. `within: changeset` requires the satisfying edges
to be created by the same changeset. A triggered rule whose `require`
does not hold is a schema violation, and the fold rejects the
changeset (§3.2.4). The same attribute-condition grammar serves policy
selectors as the `where` key (§4.1).

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

**Term identity.** Term names are graph-global: a term's identity is
its name, not the package that declared it. Admitting a term that
already exists merges the declarations — the term's parent set becomes
the union, and the merge MUST leave the taxonomy acyclic, or the
admitting changeset is rejected. Union only adds ancestors, and
requirements only accumulate (§4.1), so installing another package can
only tighten the policy surface of a shared term, never loosen it.
Packages that must not interact use disjoint roots.

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
   export a subgraph (Part 7), or publish it under a `public` grant
   (§9.4). An agent that evolved a domain ontology can hand that
   world-model to another agent without sharing any of the private data
   classified under it.
2. **Schema changes have provenance.** The graph records who added an
   entity type, when, and from what discussion it derives, using the same
   lineage machinery as any node.
3. **Schema changes are governed.** An ontology bump is a changeset,
   admitted under the current policy (§4.6). A graph can require elevated
   review for schema mutations, and the reference policy in Appendix C
   does.
