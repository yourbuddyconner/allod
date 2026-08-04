# Schema-as-Object Materialization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move schema (ontology types, taxonomy terms, validation rules, policy) into the graph as governed meta-typed nodes; derive the Registry from folded state per revision; retire `.allod/schema/`.

**Architecture:** A compiled-in **meta-registry** (the types that describe types) seeds every registry derivation — it is spec-defined and never stored. Schema elements are ordinary `node` objects of types `meta/EntityType`, `meta/EdgeType`, `meta/Struct`, `meta/TaxonomyTerm`, `meta/ValidationRule`, `meta/Policy`, each carrying typed identity attributes plus the element's canonical YAML as a `definition: string` attribute (full structural modeling of the recursive grammars is future work — this keeps per-element governance and readable diffs without the recursion rabbit hole). The fold owns registry derivation incrementally: validate changeset N against the registry as of N's parent; rebuild the registry only when a changeset touched meta nodes. `schema_context` becomes the Merkle state hash of the meta subgraph at the parent revision. Genesis compiles profile YAML into schema-node create ops. This is a deliberate behavior-changing sub-project: spec bumps v0.3 → v0.4, demo logs and affected vectors regenerate; projection package hashes and allod-lint stay untouched.

**Tech Stack:** Rust 2021 (same workspace), existing test infra, wasm rebuild at the end.

## Global Constraints

- Spec: docs/specs/2026-08-03-schema-materialization-design.md — its 6 locked decisions and 5 exit criteria govern.
- `.allod/schema/` must not exist for new graphs after Task 5; existing draft graphs are NOT migrated (throwaway per spec decision 2).
- Package content hashes in spec/vectors/vectors.yaml `packages:` and all allod-lint behavior over `ontologies/` are INVARIANT.
- The meta-registry is compiled in (`allod-core`), never serialized into any log.
- Library never prints; `AllodError` at the allod-graph layer; `Held` is Ok.
- Zero warnings (`cargo build --workspace` AND `cargo test --workspace --no-run`); every task ends with `cargo test --workspace` green.
- CLI output may change where the mechanism genuinely changed (this sub-project is behavior-changing) but every change must be deliberate: update mvp.rs/demo.rs assertions in the same commit and list each output change in the task report.
- Commit style: sentence case, imperative.

---

### Task 1: The meta-registry and `Registry::from_state`

**Files:**
- Create: `crates/allod-core/src/meta.rs`
- Modify: `crates/allod-core/src/lib.rs` (add `pub mod meta;`), `crates/allod-core/src/registry.rs`

**Interfaces (produces):**

```rust
// meta.rs
/// The spec-defined meta-ontology: the fixed point. Never stored in a log.
pub const META_PACKAGE: &str = "meta";
pub const META_TYPES: &[&str] = &[
    "meta/EntityType", "meta/EdgeType", "meta/Struct",
    "meta/TaxonomyTerm", "meta/ValidationRule", "meta/Policy",
];
/// A Registry pre-seeded with the meta package only.
pub fn meta_registry() -> Registry;
/// True when a node payload's type ref (version-stripped) is a meta type.
pub fn is_meta_type(type_ref: &str) -> bool;

// registry.rs
impl Registry {
    /// Derive a full registry from folded state: seed with meta_registry(),
    /// then walk live, non-deleted nodes whose type is a meta type, parsing
    /// each node's `definition` attribute (canonical YAML for that element)
    /// and its identity attributes (`name`, `package`, `version`, plus
    /// `status` for terms) into packages/taxonomies. meta/Policy nodes are
    /// skipped here (policy is read separately). Returns Err on any
    /// unparseable definition, naming the node id.
    pub fn from_state(state: &crate::fold::State) -> Result<Registry, String>;
}
```

Meta type attribute schemas (in `meta_registry()`, expressed with the existing grammar): every meta type has `name: string` (required), `package: string` (required except TaxonomyTerm which uses `taxonomy: string` required), `version: int`, `definition: string` (required); `meta/TaxonomyTerm` adds `parents: list<string>` and `status: enum<active|deprecated>`; `meta/Policy` has just `name` + `definition`. The `definition` for an entity type is the YAML mapping that `register_ontology` reads per type (its attributes/extends/constraints); for a term, the term's mapping; for a policy, the whole policy document. `from_state` reassembles `Package`/`Taxonomy` structures exactly as `register_ontology`/`register_taxonomy` would have (share code: extract the per-element insertion out of those two functions and call it from both paths so projection-loading and state-derivation cannot drift).

- [ ] **Step 1:** Failing tests in `meta.rs`: (a) `meta_registry()` resolves `meta/EntityType` via `resolve_type` and its `collected_attrs` include `name`/`definition` required; (b) `is_meta_type("meta/EntityType@1")` true, `"core/Person"` false; (c) round-trip: build a `State` by hand containing one `meta/EntityType` node for `core/Person` (definition = the YAML snippet from `ontologies/core/ontology.yaml`) and one `meta/TaxonomyTerm` for `workspace/scratch` with parent `workspace`; `Registry::from_state` resolves `core/Person` and `term_exists("workspace/scratch")` with correct closure.
- [ ] **Step 2:** Implement; `cargo test -p allod-core` green; workspace green (nothing else consumes it yet).
- [ ] **Step 3:** Commit: `Add the meta-registry and Registry::from_state`

---

### Task 2: Schema compiler and projector

**Files:**
- Create: `crates/allod-core/src/schemaops.rs` (`pub mod schemaops;` in lib.rs)

**Interfaces (produces):**

```rust
/// Compile projection-form documents (ontology/taxonomy YAML mappings plus
/// one policy document) into create operations on meta nodes. IDs are
/// minted by `mint_id` so callers control determinism (uuid4 in flows,
/// fixed ids in vectors).
pub fn compile_schema_ops(
    docs: &[(String, Value)],          // the ProfileSource docs shape
    policy: &Value,
    mint_id: &mut dyn FnMut() -> String,
) -> Result<Vec<Value>, String>;       // op Values, create-node shaped

/// Project the meta subgraph of a state back to projection-form documents:
/// the inverse of compile. Output names/ordering must be deterministic
/// (sorted), suitable for markdown-bundle export and fed bundles.
pub fn project_schema(state: &State) -> Result<Vec<(String, Value)>, String>;
```

One node per entity type, edge type, struct, validation rule, term, plus one per policy. `project_schema(fold(compile(...)))` must reproduce the input documents semantically: same registry when loaded (assert via comparing `Registry` contents), and byte-stable across two projections of the same state.

- [ ] **Step 1:** Failing round-trip test: compile the memory profile's docs (read from `ontologies/` in the test) → apply as one changeset to a `State` **using a registry from `meta_registry()`** → `Registry::from_state` equals the registry `load_docs` builds from the same docs (compare resolve/term behavior on a sampled set: `memory/Note` attrs, `workspace/scratch` closure, one validation rule present) → `project_schema` output reloads via `load_docs` into an equivalent registry.
- [ ] **Step 2:** Implement; workspace green; commit: `Add the schema compiler and projector`

---

### Task 3: The fold derives the registry incrementally

**Files:**
- Modify: `crates/allod-core/src/store.rs` (`fold`, `registry`, `policy`), `crates/allod-core/src/fold.rs` (only if a helper is needed — `apply_changeset(&Registry, &Value)` signature is unchanged)

The new `Graph::fold`: start `state = State::default()`, `reg = meta_registry()` **plus, transitionally, nothing else**; for each changeset in the chain: `state.apply_changeset(&reg, cs)?`, then if any of its operations created/updated/deleted a node whose type `is_meta_type`, recompute `reg = Registry::from_state(&state)?`. `Graph::registry()` becomes `Registry::from_state(&self.fold()?)`. `Graph::policy()` reads the live `meta/Policy` node's `definition` from state (parse to Value). `schema_docs()` is deleted once Task 5 removes its last callers — in THIS task mark it `#[doc(hidden)]` and leave callers working (they still read the legacy dir if present, so the whole suite stays green mid-migration: `fold` must fall back to the legacy path — `if state has no meta nodes after genesis && schema dir non-empty, use load_docs registry as before`). The fallback is temporary scaffolding, removed in Task 5; tag it `// TRANSITIONAL (task 5 removes)`.

- [ ] **Step 1:** Failing test (per-revision validation — spec exit criterion 3): over MemStore, genesis containing compiled schema ops (use `compile_schema_ops` directly) + owner principal; apply a changeset creating a `memory/Note`; then a schema changeset adding entity type `memory/Idea@1` (a new meta/EntityType node); then a changeset creating a `memory/Idea` node. Assert: the Idea-create validates (new registry in force); replaying the whole log validates every changeset (old ones against old registry); and inserting the Idea-create BEFORE the schema changeset fails the fold.
- [ ] **Step 2:** Implement with the legacy fallback; full workspace green (legacy graphs still work); commit: `Derive the registry from state in the fold`

---

### Task 4: `schema_context` becomes the meta-subgraph state hash

**Files:**
- Modify: `crates/allod-core/src/model.rs`, `crates/allod-graph/src/ops.rs` (build_changeset), `crates/allod-core/src/fold.rs` (expose the filtered state-hash helper next to the existing state-hash code)

```rust
// model.rs (new; keep the old docs-based schema_context, TRANSITIONAL-tagged,
// until Task 5 removes its last caller)
/// State hash over the meta subgraph only: same Merkle construction as the
/// full state hash (§1.7 domains), leaves restricted to live meta-typed
/// nodes and their tombstones.
pub fn schema_state_hash(state: &State) -> Result<String, String>;
```

`ops::build_changeset` computes the parent state (`graph.fold()?` — it is already called by admit_or_hold; accept the double fold for now and note it) and pins `schema_context = schema_state_hash(&parent_state)`; when the parent state contains no meta nodes (legacy graph), fall back to the old docs hash (TRANSITIONAL).

- [ ] **Step 1:** Failing tests: (a) two states differing only in a non-meta node have equal `schema_state_hash`; (b) adding a meta node changes it; (c) a changeset built on a materialized graph carries `schema_context == schema_state_hash(parent)`, and after a schema changeset the next changeset's `schema_context` differs.
- [ ] **Step 2:** Implement; workspace green; commit: `Pin schema_context to the meta-subgraph state hash`

---

### Task 5: Genesis materializes; the schema directory dies

**Files:**
- Modify: `crates/allod-graph/src/flows.rs` (init; add `install_package`), `crates/allod-core/src/store.rs` (delete `install_schema`, `schema_docs`, legacy fallbacks and TRANSITIONAL code from tasks 3-4), `crates/allod-graph/src/md.rs` (export projects via `project_schema`; import unchanged semantics), `crates/allod-graph/src/fed.rs` (bundle carries the meta subgraph objects instead of serialized schema docs; import admits them like any object), `crates/allod-graph/src/schema.rs` (describe already reads `graph.registry()` — now populate `TermView.status`/`version` and `EntityTypeView.version` from meta nodes, closing the task-8 ledger note from sub-project 1), `crates/allod/src/main.rs` + shims as needed

`flows::init`: bind owner into policy roles (unchanged), then genesis ops = owner User node + `compile_schema_ops(profile.docs, policy, uuid4)`. No `install_schema` calls anywhere. New flow `install_package(graph, docs, policy: Option<&Value>, by) -> Result<Admission, AllodError>` — compiles docs to ops and routes through `ops::commit` (so policy governs schema installation; under the reference policies, `schema-changes-are-serious`-class rules must match meta-type mutations — check `ontologies/memory/policy-local.yaml`'s schema rule selector: it keys on `operation: [define-type, set-policy, deprecate-term]`; ADD a selector alternative that matches `type: meta/*` mutations, updating the policy YAML files and their package hashes... **STOP — package hashes are invariant per Global Constraints.** Resolution, decided: do NOT touch `ontologies/` policy files; instead the op-kind vocabulary stays, and `compile_schema_ops` emits create ops whose **operation key remains `create`** but `policy.rs`'s `op_contexts` gains: an operation targeting a meta-typed node ALSO matches `operation: define-type` (for meta/EntityType, EdgeType, Struct), `set-policy` (meta/Policy), `deprecate-term` (meta/TaxonomyTerm update whose status becomes deprecated) selectors — mapping the §3.2.2 sugar verbs onto meta-node ops at selector-match time. Add focused policy tests for this mapping.)

`allod-wasm`: replace `install_schema` with `install_package(docs_yaml: String) -> Admission`; rebuild pkg; adjust the vitest suite.

- [ ] **Step 1:** Failing tests first at each layer: init produces a graph where `.allod/schema/` does not exist (assert store list("schema") empty), `describe()` still lists `memory/Note`, `verify` passes all three levels, and `Graph::policy()` returns the bound policy. The schema-mutation E2E (spec exit criterion 2): agent proposes a new entity type via `install_package` (or a raw meta-node create through `commit`) → Held by the schema rule mapping → owner `decide` approve → a subsequent changeset creates an instance of the new type → verify green and the decision record present.
- [ ] **Step 2:** Migrate md/fed/schema.rs call sites; delete legacy code; update mvp.rs/demo.rs assertions deliberately (list every output change); workspace green including wasm vitest (`pnpm --dir crates/allod-wasm build && ALLOD_BIN=<target/release/allod> pnpm --dir crates/allod-wasm test`).
- [ ] **Step 3:** Commit in coherent steps: `Materialize schema at genesis`, `Route package installation through admission`, `Project schema from state in bundles and exports`, `Remove the schema document directory`

---

### Task 6: Vectors and CI

**Files:**
- Modify: `crates/allod-vectors/src/main.rs`, `spec/vectors/*` (regenerated), `.github/workflows/ci.yml` only if a step needs it

Regenerate: the synthetic log's changesets now pin `schema_context` per the new rule — the generator builds a materialized genesis (fixed ids, not uuid4 — pass a deterministic `mint_id`) containing the core-ontology schema objects it needs, and `log.yaml`/`vectors.yaml` gain: the genesis-with-schema log, per-revision `schema_context` values, and one **schema-mutation vector** (a changeset adding an entity type, with its own hash + the schema_context change it causes). The `packages:` section of vectors.yaml must remain byte-identical (assert with git diff on that section in the task report). The governance vector pair keeps working (its registry now derives from the materialized genesis).

- [ ] **Step 1:** Regenerate, eyeball the diff (log.yaml changes are expected and large; packages section unchanged), commit: `Regenerate vectors for materialized schema and add the schema-mutation vector`
- [ ] **Step 2:** CI green locally end to end: `cargo test --workspace`, vectors reproduce (`generate` then `git diff --exit-code spec/vectors`), lint (`cargo run --release -p allod-lint`) unchanged output, wasm suite green.

---

### Task 7: Spec prose and version bump

**Files:**
- Modify: `spec/00-preliminaries.md` (§0.1 draft version), `spec/01-data-model.md` (§1.2 note), `spec/02-schema.md` (§2.5 now implemented; meta-ontology normative definition added as §2.6 or an appendix — list the six meta types and their attribute schemas, the definition-as-canonical-YAML decision stated honestly as v0.4's encoding with structural modeling anticipated), `spec/03-substrates.md` (§3.2.1 schema_context wording — it already says "state hash of the schema"; confirm and tighten; §3.2.2 sugar-verb mapping note), `spec/appendices.md` (Appendix H: the new vectors), `ontologies/README.md` (replace the "Until schema-as-object materialization ships" paragraph with a description of both binding forms), every "Draft v0.3"/"tracks spec v0.3" line across README.md, spec/, ontologies/ → v0.4.
- The spec docs `docs/specs/2026-08-03-schema-materialization-design.md` Status → Implemented (with deviations section if any).

- [ ] **Step 1:** Make the edits; run `cargo run --release -p allod-lint` (README/ontology changes must not break it); `grep -rn "v0.3" README.md spec/ ontologies/ docs/` returns nothing unintended.
- [ ] **Step 2:** Commit: `Spec v0.4: schema-as-object materialization is implemented`

---

## Self-review notes (applied)

- Exit criteria mapping: EC1 (init materializes, no schema dir, demos+mvp pass) → Tasks 5; EC2 (governed schema change E2E) → Task 5 step 1; EC3 (per-revision validation) → Task 3 step 1; EC4 (lint + package hashes unchanged, new vectors reproducible) → Task 6; EC5 (wasm install/describe from state) → Task 5.
- The §3.2.2 sugar-verb→meta-op selector mapping keeps `ontologies/` files and their hashes untouched — the one place the spec's "policies key on meta types" intent collides with the invariant package hashes, resolved at match time in policy.rs.
- The sub-project-1 ledger note (TermView status/version from meta nodes) is closed by Task 5.
