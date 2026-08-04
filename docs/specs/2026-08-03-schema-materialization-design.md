# Schema-as-object materialization — design

**Date:** 2026-08-03
**Status:** Implemented
**Scope:** Move the schema — ontology types, taxonomy terms, validation
rules, and policy — into the graph as governed objects, replacing the
installed-document mechanism (`.allod/schema/*.yaml`). This is the
materialization that §2.5 of the spec describes and that the ontologies
README anticipates. It is a deliberate behavior- and format-changing
sub-project: the spec version bumps (v0.3 → v0.4), demo logs and the
affected Appendix H vectors regenerate, and the "until schema-as-object
materialization ships" language is retired from the docs.

**Sequencing:** sub-project 2 of 3. Builds on the library refactor
(`2026-08-03-library-refactor-wasm-design.md`), ships before Freehold
(`~/code/freehold/docs/specs/2026-08-03-freehold-v0-design.md`), whose
`propose_ontology_change` flow becomes a true in-graph changeset because
of this work.

## Deviations from design

The implementation shipped without departing from the design in substance, but four details resolved at implementation time:

- **Genesis sentinel value.** The design noted genesis carries no parent. The implementation uses `sha256:0000000000000000000000000000000000000000000000000000000000000000` as the literal `schema_context` for all changesets whose parent state contains no meta nodes. This value is defined as `GENESIS_SCHEMA_CONTEXT` in `allod-core/src/model.rs`.
- **`definition`-as-YAML encoding.** The `definition` attribute on meta nodes stores the element's YAML serialization as a string. Structural modeling of the attribute schema and rule body fields is anticipated before v1.0 and will be a breaking format change. This is disclosed explicitly in §2.6.
- **Sugar-verb mapping at selector-match time.** The design stated "no new operation kinds" and that `define-type`, `deprecate-term`, etc. map onto `create`/`update`/`delete` on meta-typed nodes. The implementation resolves this mapping at policy selector evaluation (§4.1 / `policy.rs`), leaving the YAML package files and their hashes untouched.
- **Genesis vector carries all loaded packages.** The `materialized_log` vector in `vectors/vectors.yaml` captures genesis for a graph whose genesis includes all profile package nodes. This means the genesis changeset in the vector set is larger than a minimal genesis; the schema_context remains the all-zeros sentinel because no parent meta subgraph exists.

## Context

Today `Graph::registry()` loads one static registry from the schema
directory, policy is read from installed docs, and `init` copies profile
YAML into `.allod/schema/`. Consequences: schema changes are not
changesets (so they are not governed, signed, or provenance-carrying),
§1.2's rule that validation uses the schema in force at the changeset's
parent is approximated by a single registry, and in-graph ontology
imports cannot bind to state hashes.

## Decisions (locked)

1. **A spec-defined meta-ontology is the fixed point.** Schema elements
   are ordinary *nodes* of meta types: `meta/EntityType`,
   `meta/EdgeType`, `meta/Struct`, `meta/TaxonomyTerm`,
   `meta/ValidationRule`, `meta/Policy`. The meta-ontology itself — the
   types that describe types — is defined by the spec, versioned with
   it, and compiled into implementations (it joins the controlled
   vocabulary in `vocab.rs`). It is never stored in the log. Everything
   else, including `core` and `memory`, lives in the graph as governed
   objects with provenance.

2. **Genesis and installs compile projections into operations.** The
   YAML packages in `ontologies/` remain projections. The loader
   becomes a compiler from projection form to `create` operations on
   schema nodes. `init` builds genesis as: owner principal + the
   profile's schema objects + the policy object, in one self-admitted
   changeset (§4.6). Installing a package later is an ordinary
   changeset of schema-node creates, subject to policy like any other
   mutation. The `.allod/schema/` directory is gone for new graphs;
   existing draft graphs are throwaway and are not migrated.

3. **The registry derives from state, per revision.**
   `Registry::from_state(&State)` replaces directory loading. The fold
   validates each changeset against the registry as of its parent
   revision — implementing §1.2 exactly. Incremental: the fold reuses
   the registry until a changeset touches schema nodes, then rebuilds.
   `schema_context` (§3.2.1) becomes the state hash of the schema
   subgraph at the parent revision, which is what the field always
   claimed to be.

4. **No new operation kinds.** §3.2.2 already defines schema mutations
   as ordinary operations targeting schema objects. A new entity type
   is a `create` of a `meta/EntityType` node; a term deprecation is an
   `update` of a `meta/TaxonomyTerm` node. Policy evaluation reads the
   `meta/Policy` object from state. The reference policies' elevated
   review for schema mutations now keys on the meta types.

5. **Import binding splits by form.** Projection files keep
   content-hash imports — they are not in a graph, and `allod-lint` and
   the package-hash vectors are unchanged. In-graph imports bind to the
   schema subgraph's state hash, completing the migration the
   ontologies README anticipated. The README's caveat paragraph is
   replaced by a description of both forms.

6. **Spec prose and vectors move together.** Deliverables include: the
   §2.5-adjacent prose updates (Parts 1–3) describing materialized
   schema as implemented rather than intended, the meta-ontology's
   normative definition (new section or appendix), regenerated demo
   logs, new Appendix H vectors for a schema-mutation changeset and a
   per-revision `schema_context`, and the version bump to v0.4
   everywhere the docs state "tracks spec v0.3."

## Exit criteria

1. `allod init` produces a graph whose genesis carries its schema as
   objects; `.allod/schema/` does not exist; all three demos and
   `tests/mvp.rs` pass on the materialized mechanism.
2. A schema change — adding an entity type to a live graph — runs as a
   held proposal, is admitted by an owner decision record, and the new
   type is immediately usable by a subsequent changeset. `allod verify`
   reports the schema change's governance like any other mutation.
3. Validation is provably per-revision: a test admits an object that is
   valid under the schema at its parent, then a schema change, then
   shows a new changeset validating against the updated registry while
   replay of the old changeset still validates against the old one.
4. `allod-lint` output over `ontologies/` is unchanged. Package
   content-hash vectors are unchanged. New materialization vectors are
   generated and CI holds them reproducible.
5. The WASM package (`@allod/core`) exposes install-package and
   schema-mutation flows, and `describe`-style registry introspection
   reads from state — Freehold builds on this surface with no
   schema-directory concept anywhere.

## Testing

- The mvp.rs suite and demos, ported to the materialized mechanism —
  the regression net for "everything still works."
- Per-revision registry tests (exit criterion 3) at the fold layer.
- Governance tests: schema mutation held under the reference policy,
  admitted by decision record, rejected without one; the
  schema-mutation selector keys on meta types.
- Vector reproducibility in CI for the new vectors alongside the
  untouched package-hash vectors.
- Interop: a graph with an agent-proposed, owner-approved type, created
  from TypeScript, verifies under the Rust CLI (and the reverse).
