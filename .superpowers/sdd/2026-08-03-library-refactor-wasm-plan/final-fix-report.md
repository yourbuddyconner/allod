# Final Fix Report — library-refactor-wasm branch

Date: 2026-08-03  
Branch: worktree-library-refactor-wasm  

## Summary

All 11 items from the final whole-branch review were addressed in one fix wave.
`cargo test --workspace` green; `cargo test --workspace --no-run` zero warnings.
mvp.rs and demo.rs acceptance tests pass unchanged.

---

## Item 1 — `fed import` dropped the admission stdout (Critical)

**What changed:** `crates/allod/src/fed.rs` import path now iterates the returned
`Vec<Admission>` and calls `crate::print_admission(admission)` immediately after
the "bundle verified" line, before the "lineage:" line. Also extracts the held hash
from that loop instead of from a redundant `into_iter().next()`.

**Pin added:** `crates/allod/tests/mvp.rs` step-9 assertion block now includes
`"held as proposal"` as a required substring in the federation demo output.

**Test:** `appendix_a_acceptance` passes; `demo_flow_verifies_and_tampering_fails` passes.

---

## Item 2 — `md import` silently swallowed malformed files (Critical)

**What changed:** `crates/allod/src/md.rs` import function now checks
`report.skipped.first()` immediately after the import call. If non-empty, returns
`Err(format!("malformed file {}: {reason}", path.display()))`, restoring old CLI
abort behaviour. The round-trip branch also no longer checks `report.skipped.is_empty()`
redundantly — the guard is at the top.

**Test:** mvp.rs round-trip and edit-as-proposal paths still pass (no malformed files
in normal flow).

---

## Item 3 — `@allod/core` unpublishable (Important)

**What changed:** `crates/allod-wasm/package.json` additions:
- `"files": ["dist", "pkg", "js"]`
- `"license": "MIT OR Apache-2.0"`
- `"repository": { "type": "git", "url": "https://github.com/yourbuddyconner/allod" }`
- `"prepublishOnly": "pnpm run build"` script

**CI addition:** `.github/workflows/ci.yml` wasm job now runs `pnpm pack --dry-run`
after the TypeScript tests and asserts `dist/index.js` appears in the tarball listing.

---

## Item 4 — 14 warnings in allod-graph test targets (Important)

**What changed:**
- `crates/allod-graph/tests/common/mod.rs`: added `#![allow(dead_code)]` at top
  (idiomatic for shared test helper modules)
- `crates/allod-graph/tests/flows.rs`: removed `VerifyReport` from the import in
  `verify_full_jarvis_flow` (only `LevelResult` is used); renamed unused `note_r`
  to `_note_r` in `verify_governance_failure_has_real_reason`; removed unused
  `use allod_graph::flows::EnvelopeOutcome` from `envelope_err_untrusted_measurement`

**End state:** `cargo test --workspace --no-run 2>&1 | grep -c warning` → 0

---

## Item 5 — Vacuous governance test (Important)

**What changed:** Replaced the body of `verify_governance_failure_has_real_reason`
in `crates/allod-graph/tests/flows.rs`. New implementation:
1. Builds the full jarvis flow (principal_add → note → propose_preference → decide approve)
2. Finds the preference changeset hash (last admitted changeset)
3. Overwrites its evidence with `decisions: []\nenvelopes: []` via `graph.write_evidence`
4. Runs `flows::verify` and asserts `report.ok` is false
5. Finds the corrupted changeset entry and asserts its `governance` is `Failed(reason)`
   where reason contains `"governance FAILS"` or `"unmet"` and is not `"not reached"`

---

## Item 6 — `CheckpointEntry` erases failure causes (Important)

**What changed:** Added `pub reason: Option<String>` to `CheckpointEntry` struct in
`crates/allod-graph/src/flows.rs`. Populated with three distinct messages:
- Replay disagreement: `"checkpoint at {short_rev} disagrees with replay"`
- Bad signature: `"checkpoint signature: {e}"`
- Unknown signer: `"unknown signer {signer}"`

`crates/allod/src/main.rs` `cmd_verify` now uses `cp.reason.as_deref()` in its
error return instead of a hard-coded string.

---

## Item 7 — Hoist `print_admission` (Important)

**What changed:** Added `pub(crate) fn print_admission(admission: &Admission)` to
`crates/allod/src/main.rs` just after `fn short()`. All four command functions
(`cmd_principal_add`, `cmd_note`, `cmd_propose_preference`, `cmd_classify`) now call
`print_admission` instead of repeating the 15-line match block. The local
`print_admission` function in `crates/allod/src/md.rs` was removed; md.rs and
fed.rs now call `crate::print_admission`. Output is byte-identical.

---

## Item 8 — Doc line on `MemStore::load` (Minor)

**What changed:** Added three-line doc comment to `MemStore::load` in
`crates/allod-core/src/docstore.rs` explaining merge-not-replace semantics and
last-write-wins for duplicate keys.

---

## Item 9 — Rename test + `Degraded` comment (Minor)

**What changed:**
- Renamed `envelope_degraded_no_evidence` → `envelope_err_untrusted_measurement` in
  `crates/allod-graph/tests/flows.rs`; replaced the now-inaccurate comment with one
  that describes what the test actually exercises (untrusted-measurement error path)
- Added doc comment on `EnvelopeOutcome::Degraded` in `crates/allod-graph/src/flows.rs`
  noting it is currently unreachable via `flows::envelope`

---

## Item 10 — `fed::verify_bundle` tuple destructuring (Minor)

**What changed:** In `crates/allod-graph/src/fed.rs`, `verify_bundle` now
destructures the inner tuple as `let (_, source_graph, state_hash, object_count) =`
instead of binding `peer_key` and immediately discarding it with `let _ = peer_key`.

---

## Item 11 — Spec reconciliation, decision 5 (Minor)

**What changed:** `docs/specs/2026-08-03-library-refactor-wasm-design.md` decision 5
WASM v1 surface list updated to match what actually shipped: removed `"checkpoints,
registry introspection, schema-document install, the memory flows as reference sugar,
markdown bundle export/import, and federation bundle/import"` from the v1 list.
Added closing sentence: "The remaining bindings (checkpoint, envelope, markdown import,
federation) land with the Freehold sub-project that needs them."

---

## Test commands run

```
cargo test --workspace --no-run   # zero warnings
cargo test --workspace             # all test result: ok
pnpm --dir crates/allod-wasm test  # memory-flow suite: 2 passed
                                    # interop suite: pre-existing PATH failure
                                    #   (cargo not in env when vitest spawns)
```

The wasm interop failures (`spawnSync cargo ENOENT`) are pre-existing: they fail on
the same commit before these changes. They require the CI `ALLOD_BIN` env var or PATH
fix, which is out of scope for this fix wave.
