# Task 2 Report: Schema compiler and projector

## Deliverable

**File created:** `crates/allod-core/src/schemaops.rs`  
**Module registered in:** `crates/allod-core/src/lib.rs` (`pub mod schemaops;`)

## Interfaces implemented

```rust
pub fn compile_schema_ops(
    docs: &[(String, Value)],
    policy: &Value,
    mint_id: &mut dyn FnMut() -> String,
) -> Result<Vec<Value>, String>

pub fn project_schema(state: &State) -> Result<Vec<(String, Value)>, String>
```

## Tests (6 total, all green)

| Test | What it verifies |
|------|-----------------|
| `compile_round_trip_memory_profile_registry` | Compile real memory-profile docs → apply to State → `Registry::from_state` has `memory/Note` (content+title attrs), `workspace/scratch` with correct closure, `memory` imports `core` |
| `project_schema_reloads_into_equivalent_registry` | `project_schema` output fed into `load_docs` produces a registry with same type/term resolution |
| `project_schema_is_byte_stable` | Two projections of same state produce identical YAML |
| `compile_minimal_ontology_produces_correct_ops` | Minimal ontology → exactly one EntityType + one Policy op |
| `compile_taxonomy_produces_term_ops` | Taxonomy → one TaxonomyTerm op per term |
| `project_schema_naming_convention` | Projected doc names: `"core"`, `"memory"`, `"memory-taxonomy"`, `"policy"` |

## Key design decisions

**Op shape:** `{ create: { kind: "node", id, type: "meta/EntityType@1", attributes: {...} } }` — built as plain `serde_yaml::Value` mappings without depending on `allod-graph`.

**Imports encoding:** Each meta node for a package carries `imports: list<string>` (bare package names). `Registry::from_state` deduplicates and sorts them. In projection output the imports reconstruct as `[{ ontology: "name" }]` entries matching the original YAML form.

**Doc naming:** `<package>` for ontologies, `<taxonomy>-taxonomy` for taxonomies, `"policy"` for the policy node. Matches `flows::profile_from_dir` and `profiles::embedded_profile` naming exactly.

**Determinism:** `project_schema` uses `BTreeMap` throughout; taxonomy terms sorted by name; policy nodes sorted by node id; output sorted (ontologies → taxonomies → policy).

## Commit

`f41db0c` — Add the schema compiler and projector

## Status

All green. Zero warnings. `cargo test --workspace` passes.

---

## Fix 1: Multiple policy nodes error (b6694fd)

**Finding:** `project_schema` silently dropped all but the first `meta/Policy` node.

**Fix:** Return `Err` when more than one policy node exists in the state, naming all node ids in the error message: `"multiple meta/Policy nodes: <id1>, <id2> — a graph carries one policy"`.

**Test added:** `project_schema_errors_on_multiple_policy_nodes` — constructs a state with two policy nodes and asserts the error is raised with correct node names.

**Rationale:** A graph must carry exactly one authoritative policy. Multiple policy nodes represent an inconsistent state that should fail loudly rather than silently dropping data.

---

## Fix 2: Phantom definition attribute and schema consistency (b6694fd)

**Finding 1:** The taxonomy-term compile path wrote `definition: "{}"` (~line 173 in schemaops.rs) that nothing reads.

**Finding 2:** `meta_registry()` marked `definition` REQUIRED on `meta/TaxonomyTerm` in meta.rs, creating inconsistency.

**Fix:**
1. Removed the phantom `definition: "{}"` write from `compile_schema_ops` (line 173).
2. Made `definition` optional (removed `required: true`) in `meta/TaxonomyTerm` attribute schema in `meta_registry()`.

**Rationale:** Taxonomy terms carry their content in typed attributes (`parents`, `status`); there is no YAML definition to store as `definition` is for entity/edge types and structs. Removing the phantom attribute and optional marker aligns compile path and schema registry.

**Test coverage:** Existing tests `compile_taxonomy_produces_term_ops`, `project_schema_reloads_into_equivalent_registry`, and `project_schema_is_byte_stable` all pass; no new definition-required assertion breaks because no test claimed `definition` was required for terms.

---

## Test summary

- `cargo test -p allod-core`: 40 passed
- `cargo test --workspace`: 88 tests across 6 crates, all passed
- Zero warnings
- New test: `project_schema_errors_on_multiple_policy_nodes` (multiple-policy node scenario)
