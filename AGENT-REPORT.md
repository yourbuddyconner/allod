# Agent Report: keyid-passthrough

## Status

Complete. All CI checks green.

## What was done

### 1. `crates/allod-graph/src/ops.rs`

Added `pub fn build_changeset_unsigned_with_key(graph, author_name, key_id: &str, intent, ops)`.
This skips `graph.signer()` entirely and calls `build_changeset_body` directly with the
caller-supplied `key_id`. The existing `build_changeset_unsigned` now delegates to it
after resolving the signer — no behavior change for native callers.

### 2. `crates/allod-wasm/src/lib.rs` — `commit_payload`

Added `key_id: Option<String>` as a fourth parameter (wasm-bindgen supports `Option<String>`).
When `Some`, dispatches to `build_changeset_unsigned_with_key`; when `None`, the original
`build_changeset_unsigned` path is used. Updated the doc comment accordingly.

### 3. Other payload functions with the same gap

Grepped all wasm payload functions for `.signer(` calls:

- `decide_payload` — calls `allod_graph::flows::decide_payload`, which never calls
  `.signer()`. No key_id involved at all. No gap.
- `envelope_payload` — calls `allod_graph::ops::envelope_payload_parts`, which takes
  only a name string and cs_hash; no signer resolution. No gap.

Neither function has the host-managed-key blind spot.

### 4. Tests

Added `build_changeset_unsigned_with_key_matches_signer_path_and_works_without_local_key`
to `crates/allod-graph/src/ops.rs` (in the existing `mod tests`). It:

- Asserts `author.key` is identical whether the signer path or the explicit key_id path
  is used (on a graph that has a local key).
- Asserts the with_key path succeeds on a graph whose author has NO local key registered
  (the host-managed-key scenario), and that the supplied key_id lands in `author.key`.

### 5. Gates

- `cargo test --workspace`: all tests pass (129 tests across all crates, 0 failures).
- Spec vectors: identical to `spec/vectors` — changeset body format unchanged.
- `cargo clippy --workspace -- -D warnings`: 12 pre-existing errors in `allod-core`
  (hash.rs, meta.rs, schemaops.rs, store.rs), none in the files owned by this branch.
  CI does not run clippy (confirmed via `.github/workflows/ci.yml` — only `cargo test`,
  ontology lint, vectors check, and wasm tests).
