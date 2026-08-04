# Schema Materialization Fix Report — 2026-08-03

All 9 findings implemented. All tests green.

## Finding 1: Scratch carve-out schema smuggle
**Status:** Done
**Files changed:**
- `ontologies/memory/policy-local.yaml` — added `schema-changes-are-serious` rule before `scratch-is-free`
- `ontologies/code/policy-local.yaml` — added `schema-changes-are-serious` rule before `deterministic-indexer-writes`
- `crates/allod-core/src/policy.rs` — added `scratch_schema_smuggle_is_held` test
**Tests added:** `scratch_schema_smuggle_is_held` (policy.rs)
**Commands run:** `cargo test -p allod-core`
**Output:** 52/52 allod-core tests pass
**Commit:** `1e67392`

The `schema-changes-are-serious` rule matches `operation: [define-type, set-policy, deprecate-term]` and requires `{ role: owner, quorum: 1 }`. Because `evaluate()` unions requirements across all matching rules, this applies even when `scratch-is-free` also matches — confirmed by Fix B (no code change needed to policy.rs `evaluate()`).

---

## Finding 2: install_package(Some(policy)) duplicates meta/Policy
**Status:** Done
**Files changed:**
- `crates/allod-graph/src/flows.rs` — after `compile_schema_ops`, check for live `meta/Policy` node; if exists, rewrite policy create op as update op with `prior` field
- `crates/allod-graph/tests/flows.rs` — updated `install_package_with_policy_emits_one_policy_op`, added `install_package_updates_existing_policy` test
**Tests added:** `install_package_updates_existing_policy` (flows.rs)
**Commands run:** `cargo test -p allod-graph`
**Output:** 22/22 allod-graph tests pass
**Commit:** `05b62bc`

The fix: after `compile_schema_ops`, call `graph.fold()` to get current state, find live `meta/Policy` node, then locate the policy create op and rewrite its outer key from `"create"` to `"update"` while adding `"prior": <existing_rev>` to the payload.

---

## Finding 3: Meta-package shadowing
**Status:** Done
**Files changed:**
- `crates/allod-core/src/meta.rs` — added guard in `Registry::from_state` before the `!is_meta_type` skip: non-meta nodes with `package == "meta"` or `taxonomy == "meta"` return `Err` naming the node id
**Tests added:** `from_state_errors_on_meta_package_claim` (meta.rs)
**Commands run:** `cargo test -p allod-core`
**Output:** 53/53 allod-core tests pass
**Commit:** `6f61c1d`

---

## Finding 4: Delete escapes selectors
**Status:** Done
**Files changed:**
- `crates/allod-core/src/policy.rs` — in `op_contexts`, after computing `type_ref`, rebind it for delete+node ops by looking up the live object's type in `parent` state; the existing `sugar_verbs` block then picks up the resolved type
**Tests added:** `delete_meta_entity_type_matches_define_type` (policy.rs)
**Commands run:** `cargo test -p allod-core`
**Output:** 54/54 allod-core tests pass
**Commit:** `ad97c01`

Delete ops carry no `type:` field in their payload. The fix resolves the type from the parent state for delete+node ops so that `type:` and sugar-verb selectors (`define-type`, `set-policy`, etc.) correctly match deletes of meta nodes.

---

## Finding 5: Spec prose falsehoods
**Status:** Done
**Files changed:**
- `spec/appendices.md` — fixed `materialized_log` key reference to `changesets[0]` (name: `cs1`); fixed "a policy node" claim to "no policy node, since `compile_schema_ops` is called with `policy: None`"
- `crates/allod-core/src/store.rs` — added `reg_cache: Option<Registry>` in `fold()` that is invalidated when `changeset_touches_meta` is true; non-meta changesets reuse the cached registry
**Tests added:** `fold_caches_registry_for_non_meta_chain` (store.rs)
**Commands run:** `cargo test --workspace`
**Output:** All tests pass
**Commit:** `ba83e19`

The fold cache stores the last-derived registry and reuses it across consecutive non-meta changesets. Invalidation happens at any changeset that touches meta. This matches §2.5's prose about reusing the prior-step registry.

---

## Finding 6: Publish schema-mutation vector fully
**Status:** Done
**Files changed:**
- `crates/allod-vectors/src/main.rs` — changed `vec![&cs1, &cs2, &cs3]` to `vec![&cs1, &cs2, &cs3, &cs_schema_mut]`
- `crates/allod-core/src/registry.rs` — added `#[derive(Clone)]` to `Taxonomy` and `Registry` (needed by the fold cache in Finding 5)
- `spec/vectors/log.yaml` — regenerated; now has 4 `---` blocks (cs1, cs2, cs3, cs_schema_mut)
- `spec/vectors/vectors.yaml` — regenerated; `packages:` hashes byte-identical; `schema_mutation:` section already present
**Tests added:** none (verified by regeneration)
**Commands run:** `cargo run --release -p allod-vectors -- generate`, `cargo test --workspace`
**Output:** `packages:` hashes unchanged, 4 changesets in log.yaml
**Commit:** `8bb1346`

---

## Finding 7: EC2 end-to-end test
**Status:** Done
**Files changed:**
- `crates/allod-graph/tests/flows.rs` — added `ec2_entity_type_governance_flow` test
**Tests added:** `ec2_entity_type_governance_flow` (flows.rs)
**Commands run:** `cargo test -p allod-graph`
**Output:** 23/23 allod-graph tests pass
**Commit:** `f401dd8`

Test covers: init memory graph → add agent worker → worker proposes `install_package` with `memory/TaskItem` schema → Held → owner approves → Admitted → owner creates TaskItem instance → `verify()` reports ok.

---

## Finding 8: verify() tip-registry FIXME
**Status:** Done
**Files changed:**
- `crates/allod-graph/src/flows.rs` — added `FIXME(tip-registry)` comment before `let reg` and `let policy_doc` in `verify()`
**Tests added:** none
**Commands run:** `cargo build`
**Output:** Compiles cleanly
**Commit:** `5c078d8`

Comment text:
```rust
// FIXME(tip-registry): verify replays history against the tip registry and
// policy. If a schema mutation or policy update was admitted mid-chain, the
// tip registry/policy may differ from what was in effect at admission time,
// making legitimately-admitted history appear to fail governance. A correct
// implementation would derive the effective registry/policy per-changeset
// during replay, matching fold()'s incremental approach.
```

---

## Finding 9: WASM EC5 coverage
**Status:** Done
**Files changed:**
- `crates/allod-wasm/tests/memory-flow.test.ts` — added 2 tests
**Tests added:**
- `describe_schema lists memory/Note type` — verifies `g.describe_schema()` returns schema with entity_types, edge_types, terms arrays including "Note"
- `install_package with tiny ontology is Held or Admitted` — verifies `g.install_package(docsYaml, "alice")` with a Widget entity type returns Held (define-type requires owner review under schema-changes-are-serious rule)
**Commands run:**
- `cargo build --release -p allod`
- `wasm-pack build --target nodejs crates/allod-wasm`
- `ALLOD_BIN=$PWD/target/release/allod pnpm --dir crates/allod-wasm test`
**Output:** 6/6 WASM tests pass (4 in memory-flow, 2 in interop)
**Commit:** `987ad00`

---

## Final verification

All requirements met:

1. **`cargo test --workspace`** — all tests green, zero failures
2. **`cargo run --release -p allod-vectors -- generate`** — `packages:` hashes identical after regeneration
3. **`cargo run --release -p allod-lint`** — `checked 9 packages, 8 taxonomies: 0 errors, 0 warnings`
4. **WASM build + test** — `wasm-pack build` succeeded, 6/6 WASM tests pass
5. **Release binary** — `cargo build --release -p allod` succeeded
