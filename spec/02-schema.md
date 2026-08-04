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
the motivating case — without creating full objects for them. Struct names
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
only add requirements to a shared term, never remove them.
Packages that must not interact use disjoint roots.

## 2.3 Extension and inheritance

Extension is how an agent adapts to its domain: start from a base
ontology and evolve it locally.

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
taxonomy terms, and governance policies (Part 4) are nodes in the graph,
typed by the meta-ontology defined in §2.6. Changesets create and mutate
them through the same `create`, `update`, and `delete` operations that
apply to any data node (§3.2.2). Governance applies to them like any
other mutation — the reference policy in Appendix C uses the
`schema-changes-are-serious` rule, which selects on the meta types.

Three consequences follow from this design:

1. **An ontology can be shared on its own.** To export an ontology,
   export the meta-node subgraph (Part 7), or publish it under a
   `public` grant (§9.4). An agent that evolved a domain ontology can
   share it without sharing any of the private data classified under it.
2. **Schema changes have provenance.** The graph records who added an
   entity type, when, and from what discussion it derives, using the same
   lineage machinery as any node.
3. **Schema changes are governed.** A schema mutation is a changeset,
   admitted under the current policy (§4.6). A graph can require elevated
   review for schema mutations.

The registry derives from the materialized meta-node state at the
parent revision of each changeset (§3.2.1). The fold rebuilds the
registry whenever a changeset touches meta nodes; otherwise it reuses the
registry from the prior step. This implements §1.2 exactly: validation
always uses the schema in force at the parent, not the current head.

## 2.6 The meta-ontology (normative)

The meta-ontology is the fixed point that describes itself. It declares
the six node types used to store all other schema elements. The
meta-ontology is defined here and compiled into conforming
implementations; it is never stored in a log.

**Meta types.** The package name is `meta`, version 1. The six types are:

| Type | Purpose |
|---|---|
| `meta/EntityType` | One entity type declaration from an ontology |
| `meta/EdgeType` | One edge type declaration |
| `meta/Struct` | One named struct declaration |
| `meta/TaxonomyTerm` | One taxonomy term |
| `meta/ValidationRule` | One validation rule |
| `meta/Policy` | One governance policy |

**Attribute schemas.** The normative attribute schema for each type:

`meta/EntityType`:

| Attribute | Type | Required | Notes |
|---|---|---|---|
| `name` | `string` | yes | Bare name within the package, e.g. `Person` |
| `package` | `string` | yes | Owning package name, e.g. `core` |
| `version` | `int` | no | |
| `definition` | `string` | yes | Canonical YAML mapping for the element (see below) |
| `imports` | `list<string>` | no | Cross-package import strings for this package |

`meta/EdgeType`: same attributes as `meta/EntityType`.

`meta/Struct`: same attributes as `meta/EntityType`.

`meta/ValidationRule`: same attributes as `meta/EntityType`.

`meta/TaxonomyTerm`:

| Attribute | Type | Required | Notes |
|---|---|---|---|
| `name` | `string` | yes | Qualified term name, e.g. `sensitivity/private` |
| `taxonomy` | `string` | yes | Owning taxonomy name |
| `version` | `int` | no | |
| `parents` | `list<string>` | no | Parent term names |
| `status` | `enum<active\|deprecated>` | no | |
| `definition` | `string` | no | |

`meta/Policy`:

| Attribute | Type | Required | Notes |
|---|---|---|---|
| `name` | `string` | yes | Policy name |
| `definition` | `string` | yes | Canonical YAML mapping for the policy |

**The `definition` attribute.** In v0.4, `definition` stores the element's
canonical YAML serialization as a string. The fold parses this string to
reconstruct the registry. This encoding is deliberately simple. Structural
modeling of attribute schemas and rule bodies — typed fields rather than
an opaque string — is anticipated before v1.0 and will be a breaking
format change when it ships.

**Package imports.** The `imports` attribute on `meta/EntityType`,
`meta/EdgeType`, `meta/Struct`, and `meta/ValidationRule` nodes carries
the cross-package import declarations for the owning package. When
`Registry::from_state` reconstructs a package, it collects the `imports`
values from all of the package's meta nodes, deduplicates them, and sorts
them. This allows imports to round-trip through the graph without a
separate imports object.

**The genesis bootstrap rule.** The genesis changeset carries no parent.
It validates against the schema its own operations create (§4.6). Its
`schema_context` field MUST be the all-zeros sentinel
`sha256:0000000000000000000000000000000000000000000000000000000000000000`.
This value means "no prior schema." All changesets whose parent state
contains no meta nodes also carry this sentinel.

**Import binding by form.** Two forms exist:

- *Projection files* (the YAML packages in `ontologies/`) bind imports by
  content hash: a fingerprint of the imported package's exact YAML bytes,
  verified by `allod-lint`. This binding is defined relative to the
  projection form and is unchanged by in-graph materialization.
- *In-graph imports* bind by schema-subgraph state hash: the `schema_context`
  value at the revision where the importing package's schema nodes were
  admitted. This is the natural in-graph identifier because it is derived
  from the log and verifiable by replay.
