# Task 5 Report: Genesis materializes schema; the schema directory dies

**Date:** 2026-08-03  
**Branch:** schema-materialization  
**Status:** COMPLETE

---

## Summary

Task 5 completes the central migration of the schema materialization plan.
Schema now lives in the changeset log as meta-typed nodes; the `.allod/schema/`
directory and all legacy compatibility paths are gone.

---

## Commits (in order)

| Hash | Summary |
|------|---------|
| 8502596 | Materialize schema at genesis (Task 5, step 1-2) |
| b2f0813 | Project schema from state in federation bundles (Task 5, step 3a) |
| 8876595 | Project schema from state in exports and introspection (Task 5, step 3b) |
| 8cce41b | Remove the schema document directory (Task 5, step 4) |
| 91a989c | Update WASM: replace install_schema with install_package (Task 5, step 5) |

---

## What was changed

### `crates/allod-core/src/policy.rs`

Added `sugar_verbs: Vec<&'static str>` to `OpContext` and a
`meta_sugar_verbs(bare_type, payload)` helper that maps:

- `meta/EntityType`, `meta/EdgeType`, `meta/Struct` → `"define-type"`
- `meta/Policy` → `"set-policy"`
- `meta/TaxonomyTerm` (status=deprecated) → `"deprecate-term"`

Updated `selector_matches` to accept a match if any sugar verb appears in the
policy `operation:` list. This lets existing `ontologies/*/policy-*.yaml` files
govern schema installation without any changes to their package hashes (§3.2.2).

### `crates/allod-core/src/model.rs`

- Deleted `schema_context()` (TRANSITIONAL, superseded by `schema_state_hash`)
- Added `GENESIS_SCHEMA_CONTEXT` constant (`"sha256:0000..."`) — the well-known
  sentinel returned by `schema_state_hash` when the parent state has an empty
  meta subgraph. This lets `build_changeset` pin a valid `schema_context` for
  the genesis changeset (whose parent state is empty).
- Updated the `schema_state_hash_empty_state_is_err` test to assert the sentinel
  value instead of an `Err`.

### `crates/allod-core/src/store.rs`

- Deleted `install_schema()` and `schema_docs()` methods (removed in prior
  session; only cleanup here)
- Deleted `state_has_meta_nodes()` helper (dead after TRANSITIONAL removal)
- Simplified `registry()` to `Registry::from_state(&state)` directly
- Cleaned `policy()`: no longer falls back to scanning schema docs; errors
  cleanly with "no policy installed" if no `meta/Policy` node exists
- Deleted the TRANSITIONAL legacy-docs-dir branch from `fold()` — all graphs
  now take the materialized path (speculative registry for genesis changesets)

### `crates/allod-graph/src/ops.rs`

- Removed `schema_context` import from `allod_core::model`
- Simplified `build_changeset`: removed the `has_meta` conditional; always
  calls `schema_state_hash(&parent_state)` which returns the genesis sentinel
  for the first changeset

### `crates/allod-graph/src/flows.rs`

- `init`: Replaced `install_schema` calls with `compile_schema_ops`; builds
  one atomic genesis changeset containing all schema meta-node ops + owner
  User node; self-admitted per §4.6 (no policy check on genesis)
- `install_package`: New function that compiles schema docs into meta-node ops
  via `compile_schema_ops` and routes them through the admission policy

### `crates/allod-graph/src/fed.rs`

Meta-typed nodes now travel as governed objects with Merkle proofs in the
bundle's `objects` list. The old `schema: {name: doc}` map is gone. Schema
is subject to the same proof-carrying governance as all other bundle objects.

### `crates/allod-graph/src/md.rs`

- Skip meta-typed nodes in the `export_docs` loop (they appear in
  `.allod/schema/*.yaml` via `project_schema`, not as individual `.md` files)
- Use `project_schema(&state)` instead of `graph.schema_docs()` to populate
  the `.allod/schema/` section of the exported bundle

### `crates/allod-graph/src/schema.rs`

- Build `entity_type_version` and `term_meta` HashMaps from meta nodes in
  folded state
- Populate `EntityTypeView.version`, `TermView.status`, `TermView.version`
  from meta node state instead of always returning `None`

### `crates/allod-wasm/src/lib.rs`

Replaced `install_schema(name, doc_yaml)` with `install_package(docs_yaml, by)`.
The new binding accepts a YAML mapping `{name: doc, ...}`, compiles into
meta-node ops, and returns the admission outcome. Removed the now-unused
`graph_err` helper.

---

## Test Results

```
cargo test --workspace --exclude allod-wasm
```

All 15 test suites passed. 85 tests, 0 failures. Zero warnings.

```
git diff --exit-code ontologies/ spec/vectors
```

Clean — no package hash changes.

---

## Non-obvious decisions

**`GENESIS_SCHEMA_CONTEXT` sentinel vs. erroring on empty state:**  
The previous `schema_state_hash` returned `Err("empty meta-subgraph state")`
for empty state. This blocked `build_changeset` after removing the TRANSITIONAL
`schema_docs()` fallback. The fix was to return a deterministic sentinel
(`sha256:0000...`) for the empty case — semantically "no schema yet materialized
in parent state" — which is exactly the correct `schema_context` value for the
genesis changeset. The TDD test was updated to match.

**Sugar verbs in `op_contexts` vs. compile_schema_ops:**  
The policy selector mapping (`define-type`, etc.) is applied at
selector-match time inside `policy.rs`, not by emitting different op keys
from `compile_schema_ops`. This keeps `ontologies/*/policy-*.yaml` package
hashes invariant.

**Genesis self-admission:**  
`flows::init` skips the policy admission check and calls
`graph.append_changeset` directly. This is correct per §4.6: genesis is
self-admitting. The `fold()` speculative registry handles the bootstrap
problem (schema ops and User node in same changeset).
