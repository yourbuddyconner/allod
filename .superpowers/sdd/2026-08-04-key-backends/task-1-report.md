# Task 1 Report: `allod-core::keys`

## What was built

`crates/allod-core/src/keys.rs` — new module containing:

- **`KeyHandle`** enum: `File { path, name }` and `#[cfg(target_os = "macos")] Keychain { account, name }` with `name()` and `describe()` methods.
- **`KeyBackend`** trait: `id()`, `resolve()`, `sign()`, `public()`.
- **`graph_dir_component()`**: strips `sha256:` prefix, maps chars outside `[A-Za-z0-9._-]` to `'-'`.
- **`FileBackend`**: `create_dir` + `fallbacks`, with `platform_default()` (env: `ALLOD_KEYS_DIR` > `XDG_DATA_HOME/allod/keys` > `~/.local/share/allod/keys`), `store()` (creates parent dirs, refuses to overwrite), and `KeyBackend` impl that reads/decodes YAML files.
- **`Signer<'a>`**: enum-backed (`Local(Keypair)` or `Backend { backend, handle }`), with `local()`, `from_backend()`, `name()`, `sign()`, `public_hex()`, `key_id()`. Backend `key_id()` reuses `plain_sha256` + `hex_decode` from `crate::hash`.

`crates/allod-core/src/lib.rs` — added `pub mod keys;`.

## Test evidence

Command: `cargo test -p allod-core`

```
running 68 tests
test keys::tests::graph_dir_component_sanitizes ... ok
test keys::tests::file_backend_reads_legacy_fallback ... ok
test keys::tests::signer_local_and_backend_parity ... ok
test keys::tests::file_backend_creates_resolves_signs ... ok
[64 pre-existing tests] ... ok

test result: ok. 68 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s
```

## Deviations

None. All signatures match the brief exactly.

## Concerns

None.

## Fix pass (code-review follow-up)

Two issues identified in review and fixed in a single pass:

**1. TOCTOU race in `FileBackend::store` (important)**

The original `path.exists()` check followed by `fs::write` was not atomic — another process could create the file between the check and the write, defeating the no-overwrite guarantee.

Replaced with `std::fs::OpenOptions::new().write(true).create_new(true).open(&path)`. The OS `O_CREAT | O_EXCL` flag makes the existence check and file creation a single atomic syscall. An `AlreadyExists` error is mapped to the same human-readable message as before; all other errors surface as-is.

Changed lines: `crates/allod-core/src/keys.rs`, `FileBackend::store` body.

**2. Unused `handle` warning in `file_backend_creates_resolves_signs` test (minor)**

Renamed `let handle = ...` to `let _handle = ...` to silence the compiler warning. The variable is intentionally bound (to test that `store` succeeds) but not otherwise used in the test body.

**Test command and result:**

```
cargo test -p allod-core

running 68 tests
test keys::tests::graph_dir_component_sanitizes ... ok
test keys::tests::file_backend_reads_legacy_fallback ... ok
test keys::tests::signer_local_and_backend_parity ... ok
test keys::tests::file_backend_creates_resolves_signs ... ok
[64 pre-existing tests] ... ok

test result: ok. 68 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s
```

No warnings emitted.
