# Task 4 Report: `schema_context` becomes the meta-subgraph state hash

## Summary

Task 4 is complete. `schema_context` in `build_changeset` is now pinned to the Merkle hash of the meta subgraph's state entries on materialized graphs, with a TRANSITIONAL docs-hash fallback for legacy graphs.

## Changes

### `crates/allod-core/src/model.rs`

- Added `use` imports for `fold::State`, `meta::is_meta_type`, `bare`, `get_str`.
- Extracted `state_root_filtered(entries, filter)` private helper from `state_root`; `state_root` now delegates to it with a pass-through filter. Both use identical `state-leaf`/`state-node` domain separation — no duplication of Merkle logic.
- Added `pub fn schema_state_hash(state: &State) -> Result<String, String>`: collects all node-kind entries (live and tombstone) whose type is a meta type, ordered by BTreeMap order (kind + logical ID), then calls `state_root_filtered`. Returns `Err` if the meta subgraph is empty.
- Tagged existing `schema_context(docs)` with `// TRANSITIONAL (task 5 removes)`.
- Added three TDD tests:
  - `schema_state_hash_ignores_non_meta_nodes` — two states differing only in a non-meta node produce equal hashes.
  - `schema_state_hash_changes_when_meta_node_added` — adding a meta node changes the hash.
  - `schema_state_hash_empty_state_is_err` — empty meta subgraph returns Err.

### `crates/allod-graph/src/ops.rs`

- Updated import: `schema_state_hash` added alongside `changeset_hash, schema_context`.
- Updated `build_changeset`: calls `graph.fold()` to obtain the parent state (accepted double-fold, documented with comment referencing the FIXME in `store.rs`). If the parent state contains any live meta-typed node → `schema_context = schema_state_hash(&parent_state)`. Otherwise → TRANSITIONAL fallback to `schema_context(&graph.schema_docs()?)`.
- Added `#[cfg(test)] mod tests` with test `build_changeset_pins_meta_subgraph_state_hash`:
  - Builds a MemStore graph with a genesis changeset installing a `meta/EntityType@1` node.
  - Calls `build_changeset` and asserts `schema_context` field equals `schema_state_hash(parent_state)`.
  - Appends a second schema changeset and asserts the next `build_changeset`'s `schema_context` differs and matches `schema_state_hash(new_state)`.

## Test Results

- All 45 `allod-core` unit tests pass (including 3 new `schema_state_hash` tests).
- All 1 new `allod-graph` ops test passes.
- All 16 `allod-graph` integration tests pass.
- Zero warnings.
- Zero failures across the full workspace.

## Concerns / Notes

- The double-fold (once in `build_changeset`, again in `admit_or_hold`) is accepted per the task brief with a comment pointing to the existing FIXME in `store.rs`. Task 5 can collapse both into a single fold.
- `schema_state_hash` returns `Err` on an empty meta subgraph (no meta objects at all). The TRANSITIONAL fallback in `build_changeset` handles this gracefully for legacy graphs with no meta-typed nodes.
- The `state_root_filtered` helper is private (module-internal) — `state_root` remains the public entry point for full-state hashing, preserving the Appendix H vector compatibility contract.
