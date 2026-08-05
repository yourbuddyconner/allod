# Task 2 Report: Graph Backend Chain

## What was built

### `KeyBackend` trait extension (`crates/allod-core/src/keys.rs`)
- Added `store_keypair` default method to `KeyBackend` trait (returns error by default)
- Implemented `store_keypair` for `FileBackend` by delegating to `self.store(graph_id, kp).map(|_| ())`

### `Graph` struct changes (`crates/allod-core/src/store.rs`)
- Added `key_backends: Vec<Box<dyn KeyBackend>>` field to `Graph`
- Added `default_backends(dir)` free function: builds `FileBackend::platform_default` with `dir/.allod/keys` as legacy fallback
- `Graph::create` and `Graph::open` now build the default chain
- `Graph::open` parses optional `key_backends` override from `graph.yaml` (`"file"` → platform_default; anything else → error)
- `Graph::with_store` and `Graph::open_with_store` get empty chain (their `signer()` falls through to `load_key` → `Signer::local`)
- Added `set_key_backends`, `signer()`, `create_key()` methods

### `signer()` resolution chain
1. Each backend's `resolve()` tried in order
2. On success: `Signer::from_backend(backend, handle)`
3. Fallback: `load_key(name)` → `Signer::local(kp)` (in-store `.allod/keys/`)
4. If all fail: error with backend count

### `build_changeset` migration (`crates/allod-graph/src/ops.rs`)
- Signature changed from `&Keypair` to `&allod_core::keys::Signer`
- Uses `signer.name()`, `signer.key_id()`, `signer.sign(&hash)` internally
- `commit`, `signed_envelope`, `commit_with_envelope` now call `graph.signer()` internally

### Call site migrations
- `flows.rs`: `init` (keeps `save_key` for genesis—no graph_id exists yet; signer falls through to `load_key`), `principal_add`, `note`, `propose_preference`, `decide`, `classify`, `checkpoint`, `envelope`
- `md.rs`: `import`
- `fed.rs`: `peer_add`, `grant`, `revoke`, `make_bundle`, `import`
- `repo.rs`: `import_commit`
- `allod/src/main.rs`: `graph_a.load_key("conner")?.public_hex()` → `graph_a.signer("conner")?.public_hex()?`

### Hermetic test infrastructure
- Added `hermetic_keys_for_tests()` to `allod-graph/src/lib.rs` (OnceLock, per-process temp dir)
- Called in `verify_report_is_stable_across_substrate_rewire` (the only allod-graph test that uses `Graph::create`)
- Integration test subprocesses (`demo.rs`, `mvp.rs`, `gitcmd.rs`) now pass `ALLOD_KEYS_DIR` env to prevent writes to `~/.local/share/allod/keys`

## Test evidence

```
test store::tests::signer_resolves_xdg_then_legacy_then_store ... ok

test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

Full workspace: 25 `test result: ok.` lines, 0 failures.

XDG dir count: stable at 14 before and after the hermetic test run (no new writes).

## Deviations from brief

1. **Genesis uses `save_key` not `create_key`**: `flows::init` generates the owner keypair before `write_meta` is called, so there's no graph_id yet. Calling `create_key` would store the key under an empty graph_id component, making `signer()` unable to find it when called later with the real graph_id. Keeping `save_key` for genesis means the key lands in `.allod/keys/<name>.yaml` and is found via `signer()`'s `load_key` fallback. This matches the stated design: `with_store`/`open_with_store` graphs (empty chain) fall through to `load_key` fallback.

2. **`hermetic_keys_for_tests` placement**: The spec said "call at top of every filesystem-graph test". Only one allod-graph test actually uses `Graph::create` (`verify_report_is_stable_across_substrate_rewire`); the rest use `Graph::with_store` (MemStore, empty chain). Integration tests use subprocess binary calls and needed `ALLOD_KEYS_DIR` injected into the `Command` env instead.

## Concerns

None. The `Keypair` type doesn't implement `Clone`, which required using two separate `Keypair::generate` calls in the ops.rs test rather than cloning — this is correct behavior (keypairs should not be duplicated silently).

---

# Task 2 Second Fix Pass — Genesis Admission Ordering

## Problem

`flows::init` called `append_changeset` (admission) before `write_meta` and `create_key`.
The comment at the old line 150 claimed the correct ordering but the code did the opposite.
If `create_key` failed after admission, the graph would have an admitted genesis changeset
with no accessible owner key — an irrecoverable state.

## Fix (`crates/allod-graph/src/flows.rs`)

Reordered the three calls so admission is last:
1. `build_changeset` → computes `hash` (no change, was already first)
2. `graph.write_meta(&hash, ...)` — establishes graph_id in graph.yaml
3. `graph.create_key(&kp_clone)` — writes owner key to XDG backend
4. `graph.append_changeset(&cs, &hash, None)` — genesis admission (point of no return)

The hash is known after `build_changeset`, so `write_meta` can use it before admission.

## New test (`crates/allod-graph/tests/flows.rs`)

Added `init_create_key_failure_leaves_no_head`:
- Sets `ALLOD_KEYS_DIR` to a regular file (not a directory) so `FileBackend::store`'s
  `create_dir_all` returns ENOTDIR, causing `create_key` to fail.
- Asserts `flows::init` returns `Err`.
- Asserts `graph.head()` is `None` — no changeset was admitted before the failure.

## Test results

`cargo test --test flows -- init_writes_owner_key_to_xdg_backend_not_repo init_create_key_failure_leaves_no_head`:
2 passed, 0 failed.

`cargo test --workspace`: all 25 test suites ok, 0 failures.

---

# Task 2 Fix Pass — Code-Review Findings Resolved

## CRITICAL 1 — `crates/allod-graph/src/flows.rs`: Genesis key ordering

**Problem**: `flows::init` called `graph.save_key(&kp)` (in-repo `.allod/keys/`) _before_ `write_meta`, so the genesis owner key always bypassed the XDG backend.

**Fix**: Removed the premature `save_key` call. The flow now:
1. Generates `kp`, extracts `key_record` before any move.
2. Clones kp via `Keypair::from_yaml(&kp.to_yaml())` (no `Clone` impl on `Keypair`).
3. Consumes original `kp` into `Signer::local` for `build_changeset`.
4. Calls `graph.write_meta(...)` to establish the graph_id.
5. Calls `graph.create_key(&kp_clone)` — now the XDG path resolves correctly.

**New test** `init_writes_owner_key_to_xdg_backend_not_repo` in `crates/allod-graph/tests/flows.rs`: creates a filesystem graph with `ALLOD_KEYS_DIR` pointing to a temp dir, runs `flows::init`, and asserts:
- Key file is present at `$ALLOD_KEYS_DIR/<graph-id-component>/alice.yaml`
- Key file is absent at `<graph_dir>/.allod/keys/alice.yaml`

## CRITICAL 2 — `crates/allod/src/gitcmd.rs:216,251`

**Problem**: `cmd_git_decide` used `graph.load_key(&principal)` + `kp.sign(&payload)` (infallible Keypair sign, no backend delegation).

**Fix**: Replaced with `graph.signer(&principal).map_err(|e| e.to_string())?` + `signer.sign(&payload).map_err(|e| e.to_string())?`.

## IMPORTANT 3 — `crates/allod-graph/tests/fed.rs:44,158`

**Problem**: Two call sites used `graph_a.load_key("o").expect(...).public_hex()` bypassing the backend.

**Fix**: Both migrated to `graph_a.signer("o").expect("signer o").public_hex().expect("public_hex o")`.

## IMPORTANT 4 — `crates/allod-vectors/src/main.rs`

**Problem**: Private `build_changeset` in `allod-vectors` took `&Keypair` and called the infallible `kp.sign(...)`.

**Fix**: Changed signature to `&allod_core::keys::Signer<'_>`, updated the sign call to `signer.sign(&hash)?`. Callers updated: `owner` and `agent` are now `Signer::local(...)` wrappers; raw keypairs `owner_kp`/`agent_kp` kept around for `key_id()`/`public_hex()` lookups. The `decision` closure updated to take `&allod_core::keys::Signer<'_>`. Vector output is byte-identical (same keys, same signatures).

## MINOR 5 — `crates/allod-core/src/store.rs:588`

**Problem**: `signer_resolves_xdg_then_legacy_then_store` called `std::env::set_var` without synchronization, racing parallel tests.

**Fix**: Added `static SIGNER_TEST_LOCK: OnceLock<Mutex<()>>` in the test module; test acquires the lock before setting the env var. Temp dir now incorporates `subsec_nanos()` for additional uniqueness.

## Test results

`cargo test --workspace`: all `test result: ok.` — 0 failures across 25 test suites, total 135+ individual tests. `~/.local/share/allod/keys` does not exist (no real XDG writes).
