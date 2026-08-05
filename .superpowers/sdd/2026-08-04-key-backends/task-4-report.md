# Task 4 Report: `allod key` subcommands + init gitignore

**Status:** complete  
**Commit:** 6131b3d  
**Branch:** worktree-key-backends-spec  

## What was done

### Test file
Created `crates/allod/tests/key_cli.rs` verbatim from the brief, with one necessary adaptation: the `allod()` helper sets `current_dir(repo_root())` so the binary resolves `ontologies/` (the default schema path) during `allod init`. Without this the test fails immediately at the schema-load step before reaching anything the brief tests.

### Cargo.toml
Added `allod-core` and `serde_yaml` to `[dev-dependencies]` in `crates/allod/Cargo.toml` as required by the test.

### `cmd_init_profile` (main.rs)
After `flows::init` returns, writes `<dir>/.allod/.gitignore` with `keys/\n`. The `.allod/` directory already exists at that point (created by `Graph::create`), so no extra dir-creation is needed.

### `cmd_key` / `cmd_key_where` / `cmd_key_migrate` (main.rs)
Three new functions following the `cmd_git` pattern:
- `cmd_key` dispatches on the first positional arg (`where` or `migrate`).
- `cmd_key_where`: opens the graph, extracts `graph_id` from meta, constructs `FileBackend::platform_default(vec![<dir>/.allod/keys])`, calls `backend.resolve()`, and prints `handle.describe()` (which already includes the `"file: "` prefix, satisfying the `"file:"` assertion in the test).
- `cmd_key_migrate`: reads the legacy file from `<dir>/.allod/keys/<principal>.yaml`, loads the keypair, stores it via `FileBackend::platform_default(vec![])` (no fallbacks to avoid circular reads), verifies re-resolve via `graph.signer(principal)`, then deletes the legacy file. Prints `moved <from> -> <to>`. `--to keychain` stub prints the appropriate error per platform.

Added `use allod_core::keys::KeyBackend as _;` to bring `resolve()` into scope.

## Test results

```
cargo test -p allod     → all 5 test binaries pass (mvp, demo, gitcmd, key_cli, unit)
cargo test --workspace  → all crates pass, 0 failures
```

## Concerns

None. The `cmd_key_migrate` re-resolve step uses `graph.signer()` which hits the backend chain (including the fallback path). After the legacy file is deleted and the key is at the XDG path, `graph.signer()` resolves via the `FileBackend` in the graph's backend chain — this correctly validates the move before deletion.

---

## Review fixes (post-review pass)

Three findings from the Task 4 code review were addressed:

### 1. (Important) cmd_key_migrate: re-resolve guard used graph chain, not destination only

`graph.signer(principal)` hits the full backend chain including the legacy `.allod/keys` fallback. If the store had landed somewhere unexpected, the guard would still pass and the legacy key would be deleted. Fixed by replacing with `dest_backend.resolve(&graph_id, principal)?` — the XDG-only `FileBackend` with no fallbacks, confirming the key is specifically at the destination.

### 2. (Minor) cmd_key_where / cmd_key_migrate: unwrap_or_default() on graph_id

`unwrap_or_default()` silently produced an empty string, yielding a confusing "no key" error. Both functions now propagate a real error: `"could not read graph_id from graph.yaml"`.

### 3. (Minor) Cargo.toml: duplicate dev-dependencies

`allod-core` and `serde_yaml` appeared in both `[dependencies]` and `[dev-dependencies]`. Removed the redundant dev-dep entries; the test crate uses them transitively through `[dependencies]`.

### Test results after fixes

```
cargo test -p allod --test key_cli  → 1 passed; 0 failed
cargo test --workspace              → all crates pass, 0 failures
```
