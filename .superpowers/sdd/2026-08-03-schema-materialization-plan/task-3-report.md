# Task 3 Report: The fold derives the registry incrementally

**Branch:** schema-materialization  
**Commit:** cde2eb4 — "Derive the registry from state in the fold"  
**Date:** 2026-08-03

---

## What was implemented

### `crates/allod-core/src/store.rs`

Three methods were rewritten and two new helper functions plus a speculative-registry function were added:

#### `Graph::fold()` (mainline)
- Detects upfront whether the graph is a "meta graph" or a "legacy schema-dir graph" by checking if the schema dir has docs AND genesis does not contain meta-type ops.
- **Legacy path (TRANSITIONAL, task 5 removes):** loads docs via `load_docs`, folds with that registry — all existing graphs stay green.
- **Materialized path:** For each changeset in the chain:
  - If the changeset touches meta-typed nodes, calls `speculative_registry_for_changeset()` to pre-apply the meta ops speculatively, yielding a registry that includes schema defined within that very changeset. This allows a genesis changeset to both install schema AND create objects of those types in one atomic unit.
  - If the changeset does not touch meta types, derives the registry from committed state via `Registry::from_state(&state)` (meta_registry() when empty, full registry when meta nodes are present).

#### `Graph::registry()`
- Calls `fold()`, checks `state_has_meta_nodes()`.
- If meta nodes exist: `Registry::from_state(&fold()?)`.
- TRANSITIONAL fallback: `load_docs(schema_docs())` otherwise.

#### `Graph::policy()`
- Calls `fold()`, scans for live `meta/Policy` node, parses `definition` attribute to Value.
- TRANSITIONAL fallback: scan schema docs otherwise.

#### `state_has_meta_nodes(state)` (free function)
Predicate: true if any live non-deleted node has a meta type.

#### `changeset_touches_meta(state, cs)` (free function)
Returns true if any op in `cs`:
- Creates/updates a node whose `type` is a meta type, OR
- Deletes a node whose current type in `state` is a meta type (state-before-apply lookup).

#### `speculative_registry_for_changeset(state, cs)` (free function)
Clones state, speculatively applies meta create/update ops from `cs` (validated only against `meta_registry()`), then derives and returns `Registry::from_state()`. Silently skips invalid meta ops — they'll be caught by the real `apply_changeset`.

#### `schema_docs()` marked `#[doc(hidden)]`
With comment: `// TRANSITIONAL (task 5 removes): superseded by fold-derived registry`

---

## Test added: `store::tests::fold_derives_registry_incrementally`

Four-changeset scenario over MemStore:
1. **Genesis:** `compile_schema_ops(memory docs, policy)` + owner `core/User@1` node (same changeset — exercises the speculative-registry path).
2. **CS2:** Create `memory/Note`.
3. **CS3 (schema):** Create `meta/EntityType` for `memory/Idea@1` (one op, definition has `title: string`).
4. **CS4:** Create `memory/Idea` node.

**Asserts:**
- Full fold succeeds; `idea-1` is live in state.
- `graph.registry()` resolves `memory/Idea`.
- `graph.policy()` returns a Value with policy content.
- A variant with Idea-create BEFORE the schema changeset fails fold with an error mentioning schema resolution failure (`memory/Idea` does not resolve).

---

## Test results

```
test result: ok. 41 passed; 0 failed; 0 ignored   (allod-core)
test result: ok. 16 passed; 0 failed              (allod-graph)
test result: ok. 1 passed; 0 failed               (allod demo integration test)
```
Full workspace: all test suites green, zero new warnings, zero errors.

---

## Design decisions

1. **Speculative pre-apply for intra-changeset schema + data:** The brief says "genesis containing compiled schema ops PLUS the owner User node". Since changesets are atomic and the registry updates after each CS, the user node would fail validation (core/User unknown at genesis). Solution: speculatively apply meta ops from the same CS before validating the whole CS. This is sound because the meta ops themselves are validated against meta_registry().

2. **Legacy detection by schema-dir presence at fold time:** Rather than marking graphs with a flag, we detect the legacy path by checking if the schema dir has docs AND genesis does not carry meta ops. This is conservative — a graph that happens to have a schema dir but also has meta-genesis ops takes the materialized path.

3. **`Registry::from_state` not `clone`:** `Registry` is not `Clone` (it contains `Taxonomy` with `HashMap`). Rather than adding `Clone` derives, the fold recomputes from state (cheap for the meta_registry() base case, still correct).

---

## Concerns / notes for task 5

- The TRANSITIONAL path in `fold()` re-derives the registry from schema docs on every call to `fold()`, which triggers a second full fold pass (the legacy state computation). This is O(n) per fold call where n = number of changesets. Acceptable for now; task 5 removes it entirely.
- `speculative_registry_for_changeset` skips ops that fail `meta_bootstrap.resolve_type`. Malformed meta ops will be caught by `apply_changeset` with a clearer error. No correctness risk.
- The intra-changeset speculative approach means meta ops within a changeset are applied twice (speculatively + by `apply_changeset`). Idempotent for state; negligible overhead.

---

## Files changed

- `crates/allod-core/src/store.rs` — all changes (no other files touched)

---

## Review fix report (2026-08-03)

**Commit:** 4e53cbc — "Restrict speculative registry pre-apply to genesis changesets only"

### Findings addressed

#### 1. Critical — genesis-only speculative pre-apply

The fold loop previously took the speculative branch for **every** meta-touching changeset. Fixed by adding an `is_genesis: bool` flag (starts `true`, set to `false` after the first iteration). The speculative path (`speculative_registry_for_changeset`) is now guarded by `is_genesis && changeset_touches_meta(...)`. Post-genesis changesets that touch meta fall through to `Registry::from_state(&state)` — the parent-revision registry — exactly as the spec requires.

No existing test relied on non-genesis speculative pre-apply; the existing `fold_derives_registry_incrementally` test already exercises the correct split-changeset pattern (CS3 schema, CS4 instance), so no test splitting was needed.

#### 2. Critical test gap — same-changeset smuggle negative test

Added `store::tests::post_genesis_same_changeset_smuggle_fails`. The test builds a genesis containing schema ops, then a single post-genesis changeset that both creates a `meta/EntityType` for `memory/Idea@1` AND creates a `memory/Idea` node. Under the corrected rule this must fail fold; the test asserts `Err` with a message mentioning `memory/Idea` or schema resolution failure.

#### 3. Important — FIXME comments on double fold

Added `// FIXME: fold result could be cached per call site; callers using both fold twice` to `Graph::registry()` and `Graph::policy()`. No caching implemented now.

### Test results after fix

```
test result: ok. 42 passed; 0 failed; 0 ignored   (allod-core, up from 41)
test result: ok. 16 passed; 0 failed               (allod-graph flows)
test result: ok. 1 passed; 0 failed                (allod demo integration)
test result: ok. 1 passed; 0 failed                (allod mvp acceptance)
```
Full workspace: all suites green, zero warnings, zero errors.
