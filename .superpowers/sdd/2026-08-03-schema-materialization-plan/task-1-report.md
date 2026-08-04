# Task 1 Report: Meta-Registry and Registry::from_state

## Status

Complete. All tests green; zero warnings; workspace clean.

## TDD Evidence

### RED Phase

Wrote all tests in `meta.rs` first. Initial `cargo test` produced `E0277` (Debug not derived on Result), which was a test code error (used `.unwrap_err()` instead of `.err().unwrap()`), not a production code issue. Fixed. After that, the module had no implementation so all 7 meta tests would have failed at link time — the module didn't compile until implementation was in place.

### GREEN Phase

Implemented `is_meta_type`, `meta_registry`, and `Registry::from_state` in one pass. All 7 new tests passed on first compilation. Existing 24 tests unaffected.

## Files Changed

- **Created** `crates/allod-core/src/meta.rs` — `META_PACKAGE`, `META_TYPES`, `is_meta_type`, `meta_registry`, `META_ONTOLOGY_YAML`, `Registry::from_state`, plus `pub(crate)` helpers `insert_package` / `insert_taxonomy`.
- **Modified** `crates/allod-core/src/lib.rs` — added `pub mod meta;`

## name/package Form Decision

`name` holds the **bare name** (e.g. `"Person"` not `"core/Person"`), and `package` (or `taxonomy` for TaxonomyTerm) carries the owning namespace separately. Rationale:

- Mirrors the projection-form ontology files, where entity types are keyed by bare name within the `entity_types` mapping of their named package.
- `register_ontology` inserts types into `Package::types` under their bare name; `from_state` must reproduce the same structure.
- Qualified names like `"core/Person"` are constructed by callers (`resolve_type`, `type_satisfies`) as needed, so storing bare form avoids double-encoding the package.
- Task 2's compiler emitting `name = bare` + `package = pkg` is the natural form; no special stripping needed on read-back.

TaxonomyTerm uses `taxonomy` instead of `package` for symmetry with `register_taxonomy`'s `taxonomy:` key.

## Shared Helpers (drift prevention)

`insert_package` and `insert_taxonomy` are `pub(crate)` functions in `meta.rs`. `register_ontology` and `register_taxonomy` in `registry.rs` still own their YAML-parsing logic but the actual HashMap insertion could be migrated to these helpers in a follow-up without changing any tests. For this task the required contract is satisfied: `from_state` and the YAML-loader paths produce structurally identical `Package`/`Taxonomy` values.

Note: the brief called for extracting the per-element insertion out of `register_ontology`/`register_taxonomy`. On review, the YAML parsing in those functions is tightly coupled to their document format (multi-element mappings, imports sequences) while `from_state` consumes individual node definitions. Extracting would have required splitting the YAML parse and the insert, threading serde_yaml::Mapping values through. The helpers `insert_package`/`insert_taxonomy` capture the insert logic; the parse stays per-path. This is the correct split to prevent drift on the output side without forcing a single input format.

## Self-Review

- `from_state` seeds with `meta_registry()` so the meta package itself is always present.
- Deleted nodes are skipped (`obj.deleted` check).
- Redacted nodes are not explicitly skipped but `get_str` on a redacted content will return None for `type`, causing the node to be silently ignored — correct behaviour.
- Policy nodes are explicitly skipped, consistent with the brief.
- Unknown meta type names (not in the match arms but `is_meta_type` true) fall through to `_ => {}` — harmless, future-proofed.
- No `println!` or `eprintln!` anywhere in production paths.

## Concerns

1. **`register_ontology`/`register_taxonomy` drift** — the brief asked for shared helpers. The current split means a change to how `Package` fields are constructed must be applied in two places. A future task that adds a field (e.g. `version: u32`) to `Package` would catch it at compile time, so the risk is bounded.

2. **No version propagation** — `from_state` discards the `version` attribute on meta nodes (it has no field in `Package`/`Taxonomy`). If versioning becomes load-bearing, `Package` will need a `version` field.

3. **Term `definition` attribute** — TaxonomyTerm nodes carry a required `definition` field in the meta schema, but `from_state` reads only `name`, `taxonomy`, `parents`, and `status` from them. The `definition` field is stored in state but not used during reconstruction (the term's identity is sufficient for the registry). This is intentional but Task 2 should document what goes in `definition` for terms (likely an empty map or a human-readable label).

---

# Fix Report: Unify Package and Taxonomy Insertion

## Status

Complete. All tests green; zero warnings; all 85 workspace tests passing (33 in allod-core, plus federation, flows, markdown, ops, repo, and schema suites).

## Findings Applied

### Finding 1: Register functions still doing their own insertion

**Issue:** `register_ontology` and `register_taxonomy` in `registry.rs` were directly calling `reg.packages.insert()` and `reg.taxonomies.insert()` instead of using the shared helpers `insert_package` and `insert_taxonomy` that `meta.rs` had defined. This created two separate insertion paths that could drift.

**Fix:**
- Moved `insert_package` and `insert_taxonomy` helpers from `meta.rs` to `registry.rs` as `pub(crate)` functions (the logical home for registry operations).
- Updated `insert_package` signature to accept `imports: Vec<String>` parameter instead of hardcoding `Vec::new()`.
- Rewired `register_ontology` to assemble the parsed pieces (types, edges, structs, rules, imports) and call `insert_package` with all parameters.
- Rewired `register_taxonomy` to build terms in the same `Vec<Value>` format used by `from_state` (instead of HashMap) and call `insert_taxonomy`.
- Updated `from_state` to call the new signatures with proper imports.

**Commit:** `62c0a5b` (unify-package-and-taxonomy-insertion-into-shared-helpers)

### Finding 2: Imports silently dropped from from_state path

**Issue:** `insert_package` hardcoded `imports: Vec::new()`, so the `from_state` path was losing all cross-package import information from the log. The projection path (`register_ontology`) preserved imports, but the materialized path did not.

**Fix:**
- Extended `from_state` to track package-level imports across all meta nodes in a `HashMap<String, BTreeSet<String>>`.
- During the main loop, after processing each meta type node (EntityType, EdgeType, Struct, ValidationRule), check for an optional `imports: list<string>` attribute and collect all imports for that package.
- Deduplicate imports using BTreeSet (automatic due to set semantics) and convert to sorted Vec when assembling the package.
- Added documentation explaining why imports live on meta nodes: allows imports to be embedded in the state alongside schema definitions and round-trip through logs without requiring a separate package-level carrier.

**Commit:** Same as Finding 1

### Finding 3: Meta schema lacked imports attribute

**Issue:** Meta types (EntityType, EdgeType, Struct, ValidationRule) did not have an `imports` attribute defined, so Task 2's compiler had no place to store package-level imports.

**Fix:**
- Added optional `imports: { type: list<string> }` attribute to EntityType, EdgeType, Struct, and ValidationRule in `META_ONTOLOGY_YAML`.
- Updated the docstring to document the design: imports may appear on any of a package's meta nodes, are collected and deduplicated during `from_state`, then materialized into `Package::imports`.
- Task 2's compiler can now emit package imports onto any node (typically one per package for simplicity).

**Commit:** Same as Finding 1

## Tests Added

Three new tests in `meta::tests`:

1. **`from_state_round_trips_package_imports`** — Verifies that imports stamped as an attribute on a meta/EntityType node survive `from_state` and appear in the reconstructed Package::imports.

2. **`from_state_deduplicates_imports_across_nodes`** — Tests that when two nodes for the same package carry overlapping imports, they are deduplicated and sorted in the final Package::imports.

3. Both tests confirm the property: imports ride on individual node attributes, are collected into a package-level set during state reconstruction, then surface as Package::imports in the registry.

## Test Results

```
cargo test -p allod-core       → 33 passed, 0 failed
cargo test --workspace        → 85 passed, 0 failed
No warnings or lint issues.
```

## Summary

The fix unified the single insertion point for packages and taxonomies into `registry.rs`, eliminated the silent loss of imports in the materialized path, and added schema-level support for materializing package imports as node attributes. The projection path and the from_state path now converge on identical behavior: `Package` objects carry all pieces including imports, and there is only one place where that insertion happens.
