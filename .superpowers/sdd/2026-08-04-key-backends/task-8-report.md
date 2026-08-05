# Task 8 report

## Steps completed

**Step 1 — build:** `cargo build -q -p allod` succeeded, no output.

**Step 2 — scratch-graph dogfood:**
- `ALLOD_KEYS_DIR=/tmp/kb-dogfood-keys allod init /tmp/kb-dogfood --owner o`
  → graph b3885de7d1e5 created.
- `allod key where /tmp/kb-dogfood --as o`
  → `file: /tmp/kb-dogfood-keys/b3885de7d1e5…/o.yaml`
  Key landed under ALLOD_KEYS_DIR (XDG override), not inside the repo. ✓
- Cleaned up `/tmp/kb-dogfood` and `/tmp/kb-dogfood-keys`.

**Step 3 — spec update:**
- `docs/superpowers/specs/2026-08-04-key-backends-design.md`
  - Status line updated to: "implemented (file + keychain); YubiKey PIV deferred."
  - Deviations subsection added: Signer wrapper (additive); default creation
    backend always file; keychain ACL shipped without biometric attachment;
    no `key enroll`; wasm two-phase bindings extended beyond spec; YubiKey/WebAuthn
    not implemented.
- `README.md` — no Keys section existed; added one covering `allod key where`,
  `allod key migrate`, storage locations, ALLOD_KEYS_DIR override, and the
  .gitignore note.

**Step 4 — tests + clippy:**
- `cargo test --workspace`: all pass (0 failures across all crates).
- `cargo clippy --workspace --all-targets`: two deny-level errors ("this loop
  never actually loops") in `crates/allod-vectors/src/main.rs` lines 44 and 64
  — file touched in Task 2. Fixed: `for issue in &loaded.issues { return Err(…) }`
  → `if let Some(issue) = loaded.issues.iter().next() { return Err(…) }` in both
  functions. Clippy now exits clean (0 errors).

**Step 5 — committed.**
