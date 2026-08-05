# Key Backends Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Signing keys move out of governed repos behind a `KeyBackend` abstraction (file at XDG path + macOS Keychain), every signing call site routes through it, and the wasm crate gains a two-phase host-signing seam plus the git evaluation bindings freehold's review surface consumes.

**Architecture:** A new `allod-core::keys` module defines `KeyBackend` (resolve/sign/public), `KeyHandle`, and a `Signer` wrapper that call sites use in place of `Keypair`. `Graph` owns an ordered backend chain (macOS default `[keychain, file]`, else `[file]`), with the in-store `keys/` doc as a final fallback so wasm/mem graphs keep working unchanged. Decision-record construction is extracted into shared `allod-core::policy` builders so the CLI, native flows, and wasm all build byte-identical records; wasm exposes payload-builder/assembler pairs so hosts with keychain/hardware keys sign outside the sandbox.

**Tech Stack:** Rust (ed25519-dalek, serde_yaml), `security-framework` + `zeroize` (macOS only, target-gated), wasm-bindgen, vitest (wasm interop tests).

Spec: `docs/superpowers/specs/2026-08-04-key-backends-design.md` (this repo) and the wasm-additions section of `docs/specs/2026-08-04-governed-review-surface-design.md` (freehold repo — the "wasm additions (allod repo, crates/allod-wasm)" section is implemented here).

## Global Constraints

- Signatures remain raw ed25519 over a payload string, rendered `sig:ed25519:<hex>`; key ID stays plain SHA-256 of the raw 32-byte public key. Nothing about the graph, wire format, or verification changes.
- File backend location: `~/.local/share/allod/keys/<graph-id>/<principal>.yaml`, honoring `XDG_DATA_HOME`, with env override `ALLOD_KEYS_DIR` (highest precedence, used by tests). `.allod/keys/` stays as a read fallback; new keys are always created at the XDG path.
- Keychain item: generic password, service `allod`, account `<graph-id>/<principal>`, value = the same YAML record the file backend writes. Secret exists in process memory only for the duration of a sign/public call, then zeroized.
- Keychain code compiles only on macOS (`#[cfg(target_os = "macos")]` + target-specific dependency); Linux CI must stay green.
- Parity is a tested invariant: same inputs → byte-identical signed records across (a) file backend, (b) keychain backend (env-gated test), (c) wasm-internal signing vs host-side two-phase signing.
- `allod init` writes a `.allod/.gitignore` containing `keys/`.
- No git inside wasm; no policy logic in TypeScript. wasm bindings validate input shapes and return structured errors.
- YubiKey PIV is designed but NOT implemented in this plan (spec's deferral).
- Tests that create keys must be hermetic: never write to the real `~/.local/share/allod/keys`. Use the `ALLOD_KEYS_DIR` env override via the shared helper defined in Task 2. Keychain tests run only when `ALLOD_KEYCHAIN_TESTS=1` and use a throwaway service name, cleaning up items they create.
- Run the workspace suite with `cargo test --workspace`; wasm interop tests with `pnpm --dir crates/allod-wasm build && pnpm --dir crates/allod-wasm test`.

---

## File map

- Create: `crates/allod-core/src/keys.rs` — KeyHandle, KeyBackend trait, FileBackend, Signer (Tasks 1)
- Create: `crates/allod-core/src/keys_keychain.rs` — KeychainBackend, macOS only (Task 5)
- Modify: `crates/allod-core/src/lib.rs` — export `keys` module (Task 1)
- Modify: `crates/allod-core/src/store.rs` — Graph backend chain, `signer()`, `create_key()` (Task 2)
- Modify: `crates/allod-core/src/policy.rs` — `build_decision_record`, `attach_decider` (Task 3)
- Modify: `crates/allod-graph/src/{flows,ops,md,repo,fed}.rs` — call sites move to `Signer`; `flows::decide` split into build/apply halves (Tasks 2-3)
- Modify: `crates/allod/src/main.rs` — `allod key` subcommands, init gitignore (Task 4)
- Modify: `crates/allod/src/gitcmd.rs` — decide uses shared builders + signer (Task 3)
- Modify: `crates/allod-wasm/src/lib.rs` — two-phase seam + git bindings (Task 6)
- Modify: `crates/allod-wasm/tests/interop.test.ts` — new binding tests (Task 6)
- Create: `crates/allod-core/tests/key_parity.rs` — cross-backend parity vectors (Task 7)

---

### Task 1: `allod-core::keys` — KeyHandle, KeyBackend, FileBackend, Signer

**Files:**
- Create: `crates/allod-core/src/keys.rs`
- Modify: `crates/allod-core/src/lib.rs` (add `pub mod keys;`)
- Test: unit tests inside `keys.rs`

**Interfaces:**
- Consumes: `crate::sign::Keypair` (existing: `generate`, `from_yaml`, `to_yaml`, `sign`, `public_hex`, `key_id`, `name` field).
- Produces (later tasks rely on these exact signatures):

```rust
pub enum KeyHandle {
    File { path: std::path::PathBuf, name: String },
    #[cfg(target_os = "macos")]
    Keychain { account: String, name: String },
}
impl KeyHandle {
    pub fn name(&self) -> &str;
    /// Human-readable location for `allod key where` output.
    pub fn describe(&self) -> String; // e.g. "file: /home/u/.local/share/allod/keys/<gid>/conner.yaml"
}

pub trait KeyBackend {
    fn id(&self) -> &'static str; // "file" | "keychain"
    fn resolve(&self, graph_id: &str, principal: &str) -> Result<KeyHandle, String>;
    fn sign(&self, handle: &KeyHandle, payload: &str) -> Result<String, String>;
    fn public(&self, handle: &KeyHandle) -> Result<String, String>;
}

pub struct FileBackend {
    /// Where new keys are created: <create_dir>/<graph-id-component>/<principal>.yaml
    pub create_dir: std::path::PathBuf,
    /// Read-only fallback dirs, tried in order, layout <dir>/<principal>.yaml
    /// (the legacy in-repo `.allod/keys/` layout — NOT graph-id-keyed).
    pub fallbacks: Vec<std::path::PathBuf>,
}
impl FileBackend {
    /// ALLOD_KEYS_DIR > $XDG_DATA_HOME/allod/keys > ~/.local/share/allod/keys.
    pub fn platform_default(fallbacks: Vec<std::path::PathBuf>) -> FileBackend;
    /// Persist an in-memory keypair at the create path. Returns the handle.
    pub fn store(&self, graph_id: &str, kp: &crate::sign::Keypair) -> Result<KeyHandle, String>;
}

/// Filesystem-safe directory component from a graph id:
/// strip `sha256:` prefix if present, then map any char outside
/// [A-Za-z0-9._-] to '-'.
pub fn graph_dir_component(graph_id: &str) -> String;

pub struct Signer<'a> { /* private */ }
impl<'a> Signer<'a> {
    pub fn local(kp: crate::sign::Keypair) -> Signer<'static>;
    pub fn from_backend(backend: &'a dyn KeyBackend, handle: KeyHandle) -> Signer<'a>;
    pub fn name(&self) -> &str;
    pub fn sign(&self, message: &str) -> Result<String, String>;
    pub fn public_hex(&self) -> Result<String, String>;
    /// Plain SHA-256 of the raw public key bytes (§6.2) — matches Keypair::key_id.
    pub fn key_id(&self) -> Result<String, String>;
}
```

`Signer` internals: an enum of `Local(Keypair)` or `Backend { backend: &'a dyn KeyBackend, handle: KeyHandle }`. `Local` delegates to `Keypair` (wrapping in `Ok`); `Backend` delegates to the trait. `key_id()` for the backend variant: `crate::hash::plain_sha256(&hex_decode(public)?)` — reuse the same helpers `Keypair::key_id` uses.

`FileBackend::resolve`: try `<create_dir>/<graph_dir_component(graph_id)>/<principal>.yaml`, then each `<fallback>/<principal>.yaml`; first existing file wins; error `"no key for <principal> (searched <n> locations)"` otherwise. `sign`/`public` on a `File` handle: `Keypair::from_yaml(read)` then sign/public; on a `Keychain` handle return `Err("file backend cannot use a keychain handle")`. `store` creates parent dirs (`std::fs::create_dir_all`) and errors if the file already exists (never silently overwrite a key).

`platform_default`: read env vars with `std::env::var`; home dir via `std::env::var("HOME")` (this crate has no dirs dependency; that is fine — document that Windows is out of scope per the spec).

- [ ] **Step 1: Write the failing tests** (in `#[cfg(test)] mod tests` at the bottom of `keys.rs`; create the file with only the test module and `use` statements referencing the not-yet-written items so compilation fails meaningfully)

```rust
#[test]
fn graph_dir_component_sanitizes() {
    assert_eq!(graph_dir_component("sha256:ab/cd:ef"), "ab-cd-ef");
    assert_eq!(graph_dir_component("plain-id_1.2"), "plain-id_1.2");
}

#[test]
fn file_backend_creates_resolves_signs() {
    let tmp = std::env::temp_dir().join(format!("allod-keys-t1-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let be = FileBackend { create_dir: tmp.clone(), fallbacks: vec![] };
    let kp = crate::sign::Keypair::generate("alice");
    let expected_public = kp.public_hex();
    let handle = be.store("sha256:feed", &kp).unwrap();
    // Path layout: <create_dir>/feed/alice.yaml
    assert!(tmp.join("feed").join("alice.yaml").is_file());
    let resolved = be.resolve("sha256:feed", "alice").unwrap();
    assert_eq!(resolved.name(), "alice");
    assert_eq!(be.public(&resolved).unwrap(), expected_public);
    let sig = be.sign(&resolved, "sha256:00ff").unwrap();
    assert!(crate::sign::verify(&expected_public, "sha256:00ff", &sig).is_ok());
    // Missing principal errors.
    assert!(be.resolve("sha256:feed", "bob").is_err());
    // Never overwrite.
    assert!(be.store("sha256:feed", &crate::sign::Keypair::generate("alice")).is_err());
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn file_backend_reads_legacy_fallback() {
    let tmp = std::env::temp_dir().join(format!("allod-keys-t2-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let legacy = tmp.join("repo/.allod/keys");
    std::fs::create_dir_all(&legacy).unwrap();
    let kp = crate::sign::Keypair::generate("carol");
    std::fs::write(
        legacy.join("carol.yaml"),
        serde_yaml::to_string(&kp.to_yaml()).unwrap(),
    ).unwrap();
    let be = FileBackend { create_dir: tmp.join("xdg"), fallbacks: vec![legacy.clone()] };
    let h = be.resolve("sha256:anything", "carol").unwrap();
    assert_eq!(be.public(&h).unwrap(), kp.public_hex());
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn signer_local_and_backend_parity() {
    let tmp = std::env::temp_dir().join(format!("allod-keys-t3-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let be = FileBackend { create_dir: tmp.clone(), fallbacks: vec![] };
    let kp = crate::sign::Keypair::from_secret_hex(
        "dana",
        "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
    ).unwrap();
    let expected_sig = kp.sign("sha256:aa");
    let expected_kid = kp.key_id();
    let handle = be.store("gid", &kp).unwrap();
    let s_local = Signer::local(kp);
    let s_backend = Signer::from_backend(&be, handle);
    assert_eq!(s_local.sign("sha256:aa").unwrap(), expected_sig);
    assert_eq!(s_backend.sign("sha256:aa").unwrap(), expected_sig);
    assert_eq!(s_local.key_id().unwrap(), expected_kid);
    assert_eq!(s_backend.key_id().unwrap(), expected_kid);
    assert_eq!(s_local.name(), "dana");
    assert_eq!(s_backend.name(), "dana");
    let _ = std::fs::remove_dir_all(&tmp);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p allod-core keys:: 2>&1 | tail -20`
Expected: compilation FAILS (unresolved items `FileBackend`, `Signer`, `graph_dir_component`).

- [ ] **Step 3: Implement `keys.rs`** per the Interfaces block above, and add `pub mod keys;` to `crates/allod-core/src/lib.rs`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p allod-core`
Expected: PASS (new tests plus all existing allod-core tests).

- [ ] **Step 5: Commit**

```bash
git add crates/allod-core/src/keys.rs crates/allod-core/src/lib.rs
git commit -m "feat(keys): KeyBackend trait, file backend (XDG + legacy fallback), Signer"
```

---

### Task 2: Graph backend chain + all call sites move to `Signer`

**Files:**
- Modify: `crates/allod-core/src/store.rs` (Graph struct + constructors, `signer()`, `create_key()`; keep `save_key`/`load_key` as store-level primitives)
- Modify: `crates/allod-graph/src/ops.rs` (`build_changeset`, `commit`, `signed_envelope`, `commit_with_envelope`, `key_record`)
- Modify: `crates/allod-graph/src/flows.rs`, `crates/allod-graph/src/md.rs`, `crates/allod-graph/src/repo.rs`, `crates/allod-graph/src/fed.rs` (every `graph.load_key(x)` → `graph.signer(x)`, every `graph.save_key(&kp)` in creation flows → `graph.create_key(&kp)`)
- Modify: `crates/allod-graph/tests/fed.rs`, `crates/allod/src/main.rs`, `crates/allod-vectors/src/main.rs` (compile fixes; vectors use `Signer::local`)
- Test: new tests in `store.rs`; whole workspace suite

**Interfaces:**
- Consumes (Task 1): `KeyBackend`, `FileBackend::platform_default(fallbacks)`, `FileBackend::store`, `Signer`, `Signer::local`, `Signer::from_backend`, `graph_dir_component`.
- Produces:

```rust
impl Graph {
    /// Ordered backend chain. Populated by create/open from the platform
    /// default (macOS: keychain then file — keychain arrives in Task 5, so
    /// until then the default is [file] everywhere), overridden by an
    /// optional `key_backends: [<id>, ...]` list in graph.yaml.
    /// with_store / open_with_store graphs get an empty chain.
    pub fn set_key_backends(&mut self, backends: Vec<Box<dyn crate::keys::KeyBackend>>);

    /// Resolve a signer: try each backend in order with the graph_id from
    /// meta(); if none resolves, fall back to the in-store keys/ doc
    /// (load_key → Signer::local). Error only if every route fails.
    pub fn signer(&self, name: &str) -> Result<crate::keys::Signer<'_>, String>;

    /// Persist a freshly generated keypair through the first backend
    /// (FileBackend::store). If the chain is empty (in-memory graphs),
    /// fall back to save_key (in-store doc). graph_id from meta().
    pub fn create_key(&self, kp: &crate::sign::Keypair) -> Result<(), String>;
}
```

- `Graph::create(dir)` / `Graph::open(dir)` build the default chain with `FileBackend::platform_default(vec![<store root>/keys])` where `<store root>` is the `.allod` directory the store resolves to (the constructors know `dir`; the fallback path is `dir` joined with the same subpath `FsStore` uses for `keys/` — inspect `FsStore::new` in `docstore.rs` and reuse its root resolution).
- `graph.yaml` override: in `open`, after reading meta, if a `key_backends` sequence exists, build the chain in that order from ids (`"file"` → platform_default file backend; `"keychain"` → Task 5, until then return error `"unknown key backend keychain (not built on this platform)"` on non-macOS / pre-Task-5).
- `build_changeset(graph: &Graph, kp: &Keypair, ...)` changes its second parameter to `signer: &crate::keys::Signer` — inside, `kp.sign(&hash)` becomes `signer.sign(&hash)?` (map the String error into the function's existing error type) and any use of the author name uses `signer.name()`.
- `ops::key_record(&kp)` (builds the KeyRecord attribute for principal nodes) keeps taking `&Keypair` — key records are only built at generation time when the keypair is in memory.
- `flows::init` restructure (genesis ordering — graph_id is the genesis hash, so the key cannot be stored at the graph-id-keyed path before the hash exists): generate `kp` in memory → build the genesis changeset via `Signer::local`-style signing (keep using the in-memory `kp` through a `let signer = Signer::local(kp)` — note `Signer::local` takes ownership; generate public/key-record values BEFORE moving `kp` in) → compute `hash` → `graph.write_meta(&hash, ...)` (existing call) → `graph.create_key(&kp_clone_or_record)` → admit. Concretely: keep `kp` as `Keypair`, use `&kp` directly for record building and a temporary `Signer` only where `build_changeset` needs it (`build_changeset(graph, &Signer::local(kp), ...)` moves it — instead bind `let signer = Signer::local(kp);` after extracting `key_record(&kp)` and `public_hex`, and persist via a second `Keypair::from_yaml(&yaml)` clone taken before the move: `let kp_for_store = Keypair::from_yaml(&kp.to_yaml()).expect("roundtrip")`). Persist AFTER `write_meta` succeeds and BEFORE admission, so a failed store aborts init without a half-admitted graph.
- `flows::principal_add` (and `agent_add` if separate): after generating `kp`, replace `graph.save_key(&kp)` with `graph.create_key(&kp)` (graph_id exists by then via meta()).
- Every other `graph.load_key(name)` call site becomes `graph.signer(name)` with `.sign(...)` / `.public_hex()` gaining `?` (map errors with the file's existing error-conversion idiom, e.g. `.map_err(AllodError::from)`).
- **Hermetic test helper** — add to `crates/allod-graph/src/lib.rs` (or a `tests/common/mod.rs` if the crate has no test-support section):

```rust
/// Point ALLOD_KEYS_DIR at a per-process temp dir so tests never write
/// to the real XDG data dir. Safe to call from every test; first call wins.
pub fn hermetic_keys_for_tests() {
    static ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("allod-test-keys-{}", std::process::id()));
        std::env::set_var("ALLOD_KEYS_DIR", &dir);
    });
}
```

Call it at the top of every test (in allod-graph and allod-core) that creates a filesystem graph (`Graph::create` + `flows::init`). Because backends read the env at Graph construction time, the helper must run before `Graph::create`/`open`.

- [ ] **Step 1: Write the failing test** (in `store.rs` tests)

```rust
#[test]
fn signer_resolves_xdg_then_legacy_then_store() {
    // Graph in a temp dir; ALLOD_KEYS_DIR pointed at another temp dir.
    let root = std::env::temp_dir().join(format!("allod-store-signer-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::env::set_var("ALLOD_KEYS_DIR", root.join("xdg"));
    let gdir = root.join("g");
    let graph = Graph::create(&gdir).unwrap();
    graph.write_meta("sha256:cafe", &[]).unwrap();
    // (a) create_key goes to the XDG path, keyed by graph id.
    let kp = Keypair::generate("alice");
    let public = kp.public_hex();
    graph.create_key(&kp).unwrap();
    assert!(root.join("xdg").join("cafe").join("alice.yaml").is_file());
    let s = graph.signer("alice").unwrap();
    assert_eq!(s.public_hex().unwrap(), public);
    // (b) a legacy in-repo key still resolves (fallback read).
    let legacy = Keypair::generate("legacy");
    graph.save_key(&legacy).unwrap(); // store-level write to .allod/keys/
    let s2 = graph.signer("legacy").unwrap();
    assert_eq!(s2.public_hex().unwrap(), legacy.public_hex());
    // (c) unknown principal errors.
    assert!(graph.signer("nobody").is_err());
    let _ = std::fs::remove_dir_all(&root);
}
```

(Note: this test manipulates `ALLOD_KEYS_DIR`; it must set the var before `Graph::create` and other allod-core tests must not depend on that var — the Task 1 tests use explicit `create_dir` fields, so they don't.)

- [ ] **Step 2: Run to verify failure** — `cargo test -p allod-core signer_resolves` → FAIL (methods missing).
- [ ] **Step 3: Implement** Graph changes (`set_key_backends`, `signer`, `create_key`, constructor chains, graph.yaml override).
- [ ] **Step 4: Run** `cargo test -p allod-core` → PASS.
- [ ] **Step 5: Migrate call sites** across allod-graph, allod, allod-vectors per the Interfaces notes; add `hermetic_keys_for_tests()` calls to filesystem-graph tests.
- [ ] **Step 6: Run the whole workspace** — `cargo test --workspace` → PASS. Also verify no test wrote to the real data dir: `ls ~/.local/share/allod/keys 2>/dev/null` must show nothing new (or not exist).
- [ ] **Step 7: Commit**

```bash
git add -A crates
git commit -m "feat(keys): Graph backend chain; all signing call sites move to Signer; genesis stores keys at XDG path"
```

---

### Task 3: Shared decision-record builders + two-phase decide in flows

**Files:**
- Modify: `crates/allod-core/src/policy.rs` (add builders next to the existing `decision_payload` at line ~548)
- Modify: `crates/allod-graph/src/flows.rs` (`decide` at line ~398 splits into build/apply halves)
- Modify: `crates/allod/src/gitcmd.rs` (`cmd_git_decide` at line ~196 uses the shared builders + `graph.signer`)
- Test: `policy.rs` unit tests; existing flows tests must stay green; a new parity test in flows

**Interfaces:**
- Consumes: `decision_payload(record)` (existing), `policy_context(policy_doc)` (existing), `Graph::signer` (Task 2).
- Produces:

```rust
// policy.rs
/// The unsigned decision record: kind/subject/policy_context/verdict/timestamp,
/// exactly the shape flows::decide and `allod git decide` build today.
pub fn build_decision_record(
    policy_doc: &Value, subject: &str, verdict: &str, timestamp: &str,
) -> Result<Value, String>;

/// Append a decider entry {principal: "principal:<name>", signature} to
/// record.deciders (creating the sequence if absent).
pub fn attach_decider(record: &mut Value, principal: &str, signature: &str);
```

```rust
// flows.rs
/// First phase: guard already-decided, build the unsigned record and its
/// signing payload. Read-only.
pub fn decide_payload(graph: &Graph, hash: &str, verdict: &str)
    -> Result<(Value /*record*/, String /*payload*/), AllodError>;

/// Second phase: apply an externally signed record (deciders already
/// attached). Runs the guard again, then the existing satisfaction /
/// admission logic verbatim.
pub fn decide_with_record(graph: &Graph, hash: &str, record: Value)
    -> Result<DecisionOutcome, AllodError>;

/// Unchanged public signature; now = decide_payload + signer.sign +
/// attach_decider + decide_with_record.
pub fn decide(graph: &Graph, hash: &str, by: &str, verdict: &str)
    -> Result<DecisionOutcome, AllodError>;
```

Notes: the already-decided guard (flows.rs lines 417-434) runs in BOTH halves — `decide_payload` so callers fail fast, `decide_with_record` because it is the mutating gate. `decide_with_record` must validate the record's shape (kind == decision-record, subject == the proposal hash argument, at least one decider with a `sig:ed25519:` signature) and return `AllodError::Other` on mismatch. `cmd_git_decide` drops its hand-rolled record blocks (lines 218-258) in favor of `build_decision_record(&policy, &subject, &verdict, &allod_graph::ops::now_iso())` + `graph.signer(&principal)` + `attach_decider`.

- [ ] **Step 1: Write the failing tests**

In `policy.rs`:

```rust
#[test]
fn build_decision_record_matches_handrolled_shape() {
    let policy: Value = serde_yaml::from_str("rules: []").unwrap();
    let rec = build_decision_record(&policy, "git:abc123", "approve", "2026-08-04T00:00:00Z").unwrap();
    assert_eq!(rec.get("kind").unwrap().as_str().unwrap(), "decision-record");
    assert_eq!(rec.get("subject").unwrap().as_str().unwrap(), "git:abc123");
    assert_eq!(rec.get("verdict").unwrap().as_str().unwrap(), "approve");
    assert_eq!(
        rec.get("policy_context").unwrap().as_str().unwrap(),
        policy_context(&policy).unwrap()
    );
    // Payload computes on the unsigned record; attach_decider then adds a decider.
    let payload = decision_payload(&rec).unwrap();
    assert!(!payload.is_empty());
    let mut rec = rec;
    attach_decider(&mut rec, "conner", "sig:ed25519:00");
    let d = rec.get("deciders").unwrap().as_sequence().unwrap();
    assert_eq!(d[0].get("principal").unwrap().as_str().unwrap(), "principal:conner");
    assert_eq!(d[0].get("signature").unwrap().as_str().unwrap(), "sig:ed25519:00");
}
```

In `flows.rs` tests (use the same fixture style as the existing decide tests in that file — a temp graph via `hermetic_keys_for_tests()` + `init` + a held proposal):

```rust
#[test]
fn two_phase_decide_equals_internal_decide() {
    // Fixture: init graph, create a held proposal exactly as the existing
    // decide tests do (reuse that helper/fixture code).
    // Phase A on a copy: flows::decide(graph, hash, "owner", "approve").
    // Phase B on an identical second fixture: decide_payload → sign the
    // payload with graph.signer("owner") → attach_decider → decide_with_record.
    // Assert both reach DecisionOutcome::Admitted and the stored evidence
    // decision records are identical except nothing (timestamps differ —
    // so instead: build ONE fixture, call decide_payload, sign, attach,
    // decide_with_record, and assert the outcome is Admitted and the
    // evidence contains the exact record we attached.
}
```

(Write the real test following that comment: single fixture, two-phase path only, asserting the evidence file contains the attached record verbatim and the outcome is `Admitted`. The equality-with-internal-path claim is covered by Task 7's parity suite where the timestamp is pinned.)

- [ ] **Step 2: Run to verify failure** — `cargo test -p allod-core build_decision_record` and `cargo test -p allod-graph two_phase_decide` → FAIL.
- [ ] **Step 3: Implement** the builders, the flows split, and the gitcmd rewrite.
- [ ] **Step 4: Run** `cargo test --workspace` → PASS.
- [ ] **Step 5: Live check** (dogfood shape, no mutation): `cargo run -q -p allod -- git eval . HEAD --repo allod` still evaluates.
- [ ] **Step 6: Commit**

```bash
git add crates/allod-core/src/policy.rs crates/allod-graph/src/flows.rs crates/allod/src/gitcmd.rs
git commit -m "feat(policy): shared decision-record builders; flows::decide splits into payload/apply halves; git decide uses both"
```

---

### Task 4: `allod key` subcommands + init gitignore

**Files:**
- Modify: `crates/allod/src/main.rs` (subcommand dispatch around line 541; new `cmd_key_*` functions; `cmd_init` writes `.allod/.gitignore`)
- Test: `crates/allod/tests/key_cli.rs` (new integration test, `assert_cmd`-free — use `std::process::Command` with `env!("CARGO_BIN_EXE_allod")` like any existing CLI tests; if none exist, this file establishes the pattern)

**Interfaces:**
- Consumes: `Graph::open`, `Graph::signer`, `FileBackend::{platform_default, store}`, `graph_dir_component`, `KeyHandle::describe` (Task 1-2).
- Produces (CLI surface):
  - `allod key where <dir> --as <principal>` — prints the resolving backend id and `KeyHandle::describe()` output, or exits 1 with the resolve error.
  - `allod key migrate <dir> --as <principal>` — moves `<dir>/.allod/keys/<principal>.yaml` to the XDG path (`FileBackend::store` with the graph id from meta, then delete the legacy file only after the store succeeds and a re-resolve through `graph.signer` finds the new location). Prints `moved <from> -> <to>`. Errors if no legacy file exists ("nothing to migrate: no repo-local key at <path>") or if the destination exists.
  - `--to keychain` on migrate: implemented in Task 5; until then print `error: keychain backend not built yet` (exit 1) — Task 5 replaces this stub. On non-macOS, permanently errors with `keychain backend is macOS-only`.
  - `cmd_init` additionally writes `<dir>/.allod/.gitignore` with content `keys/\n` (unconditionally, overwriting is fine — the file is one line we own).

- [ ] **Step 1: Write the failing integration test**

```rust
// crates/allod/tests/key_cli.rs
use std::process::Command;

fn allod(args: &[&str], env_keys: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_allod"))
        .args(args)
        .env("ALLOD_KEYS_DIR", env_keys)
        .output()
        .expect("run allod")
}

#[test]
fn init_migrate_where_roundtrip() {
    let root = std::env::temp_dir().join(format!("allod-keycli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let keys = root.join("keys");
    let g = root.join("g");
    let g_s = g.to_str().unwrap();

    // init writes .allod/.gitignore and creates the owner key at the XDG path.
    let out = allod(&["init", g_s, "--owner", "o"], &keys);
    assert!(out.status.success(), "init failed: {}", String::from_utf8_lossy(&out.stderr));
    let gi = std::fs::read_to_string(g.join(".allod/.gitignore")).unwrap();
    assert!(gi.contains("keys/"));
    assert!(!g.join(".allod/keys/o.yaml").exists(), "key must not be repo-local");

    // where reports the file backend.
    let out = allod(&["key", "where", g_s, "--as", "o"], &keys);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(text.contains("file:"), "got: {text}");

    // Simulate a legacy repo-local key, then migrate it.
    let legacy_dir = g.join(".allod/keys");
    std::fs::create_dir_all(&legacy_dir).unwrap();
    // A fresh key under a different principal name so it doesn't collide:
    // generate via a second init in a scratch graph is overkill — write a
    // record through the library instead:
    // (integration tests may use the allod-core crate directly)
    let kp = allod_core::sign::Keypair::generate("legacy");
    std::fs::write(legacy_dir.join("legacy.yaml"),
        serde_yaml::to_string(&kp.to_yaml()).unwrap()).unwrap();
    let out = allod(&["key", "migrate", g_s, "--as", "legacy"], &keys);
    assert!(out.status.success(), "migrate failed: {}", String::from_utf8_lossy(&out.stderr));
    assert!(!legacy_dir.join("legacy.yaml").exists(), "legacy file must be moved");
    let out = allod(&["key", "where", g_s, "--as", "legacy"], &keys);
    assert!(out.status.success());

    // migrate again: nothing to do → error.
    let out = allod(&["key", "migrate", g_s, "--as", "legacy"], &keys);
    assert!(!out.status.success());

    let _ = std::fs::remove_dir_all(&root);
}
```

(Add `allod-core` and `serde_yaml` to `[dev-dependencies]` of `crates/allod/Cargo.toml` if not present.)

- [ ] **Step 2: Run to verify failure** — `cargo test -p allod --test key_cli` → FAIL (`key` is an unknown subcommand; init doesn't write the gitignore yet).
- [ ] **Step 3: Implement** the `key` dispatch arm, `cmd_key_where`, `cmd_key_migrate`, and the `cmd_init` gitignore write. Usage strings follow the existing `Err("usage: ...")` idiom in main.rs.
- [ ] **Step 4: Run** `cargo test -p allod` then `cargo test --workspace` → PASS.
- [ ] **Step 5: Commit**

```bash
git add crates/allod/src/main.rs crates/allod/tests/key_cli.rs crates/allod/Cargo.toml
git commit -m "feat(cli): allod key where/migrate; init writes .allod/.gitignore and creates keys out-of-repo"
```

---

### Task 5: macOS Keychain backend

**Files:**
- Create: `crates/allod-core/src/keys_keychain.rs` (whole file `#![cfg(target_os = "macos")]`-gated via the mod declaration)
- Modify: `crates/allod-core/src/lib.rs` (`#[cfg(target_os = "macos")] pub mod keys_keychain;`)
- Modify: `crates/allod-core/Cargo.toml`:

```toml
[target.'cfg(target_os = "macos")'.dependencies]
security-framework = "3"
zeroize = "1"
```

- Modify: `crates/allod-core/src/store.rs` (macOS default chain becomes `[KeychainBackend, FileBackend]`; graph.yaml `key_backends: [keychain, ...]` resolves)
- Modify: `crates/allod/src/main.rs` (`allod key migrate --to keychain` real implementation replacing the Task 4 stub)
- Test: env-gated tests inside `keys_keychain.rs`

**Interfaces:**
- Consumes: `KeyBackend`, `KeyHandle::Keychain { account, name }`, `Keypair` (Task 1).
- Produces:

```rust
pub struct KeychainBackend {
    /// Keychain service attribute; "allod" in production, overridable for tests.
    pub service: String,
}
impl KeychainBackend {
    pub fn new() -> Self { Self { service: "allod".into() } }
    /// Store a keypair's YAML record as a generic password
    /// (service, account = "<graph-id-component>/<principal>").
    /// Errors if an item already exists for that account.
    pub fn store(&self, graph_id: &str, kp: &crate::sign::Keypair) -> Result<KeyHandle, String>;
    /// Delete the item (used by tests and future revocation tooling).
    pub fn delete(&self, graph_id: &str, principal: &str) -> Result<(), String>;
}
impl KeyBackend for KeychainBackend {
    fn id(&self) -> &'static str { "keychain" }
    // resolve: find_generic_password(service, account) — NotFound → Err, no prompt.
    // sign: retrieve the value, parse Keypair::from_yaml, sign, then zeroize
    //       the retrieved byte buffer (zeroize::Zeroize on the Vec<u8>).
    // public: same retrieval path, return the record's public field.
}
```

Implementation notes for the brief:
- Use `security_framework::passwords::{set_generic_password, get_generic_password, delete_generic_password}`. Item value = the exact YAML string the file backend writes (`serde_yaml::to_string(&kp.to_yaml())`).
- Account attribute: `format!("{}/{}", crate::keys::graph_dir_component(graph_id), principal)` — the same sanitization as the file backend so the two agree.
- Access control: after the basic store path works, attempt the biometry-or-passcode ACL via `security_framework::access_control::SecAccessControl::create_with_protection(Some(ProtectionMode::AccessibleWhenPasscodeSetThisDeviceOnly), SecAccessControlCreateFlags::USER_PRESENCE)` and the item-add options API if the crate version exposes attaching it to a generic password. If the crate cannot attach an ACL to a generic-password add, ship without the ACL, leave the code path with a comment stating the platform still gates retrieval on keychain unlock, and record the deviation in the report (the spec's honesty note anticipates exactly this).
- `Keypair` currently offers no way to zeroize its internal `SigningKey`; the retrieved YAML string and the parsed secret buffer are what we zeroize. Dropping the `Keypair` at end of the sign call is the boundary — keep the sign body minimal so the secret's lifetime is the call.
- Tests: guarded at runtime, not compile time —

```rust
fn test_service() -> Option<KeychainBackend> {
    if std::env::var("ALLOD_KEYCHAIN_TESTS").ok().as_deref() != Some("1") { return None; }
    Some(KeychainBackend { service: format!("allod-test-{}", std::process::id()) })
}

#[test]
fn keychain_store_resolve_sign_roundtrip() {
    let Some(be) = test_service() else { return }; // skipped without opt-in
    let kp = crate::sign::Keypair::generate("kc");
    let public = kp.public_hex();
    let h = be.store("sha256:beef", &kp).unwrap();
    let sig = be.sign(&h, "sha256:11").unwrap();
    assert!(crate::sign::verify(&public, "sha256:11", &sig).is_ok());
    assert_eq!(be.public(&h).unwrap(), public);
    // Double-store errors; missing resolve errors.
    assert!(be.store("sha256:beef", &crate::sign::Keypair::generate("kc")).is_err());
    assert!(be.resolve("sha256:beef", "other").is_err());
    be.delete("sha256:beef", "kc").unwrap();
    assert!(be.resolve("sha256:beef", "kc").is_err());
}
```

- Default chain change in `store.rs`: on macOS, `vec![Box::new(KeychainBackend::new()), Box::new(FileBackend::platform_default(...))]`. Resolve order means a keychain item wins over a file when both exist (the migration story: after `--to keychain`, the file is deleted anyway).
- `allod key migrate <dir> --as <p> --to keychain`: resolve the current key through the FILE backend only (XDG or legacy), load the `Keypair`, `KeychainBackend::new().store(...)`, verify `graph.signer(p)` now resolves via keychain (`Signer` works), then delete the source file. Print `moved <path> -> keychain (service allod, account <acct>)`.

- [ ] **Step 1: Write the tests** (above; they self-skip without `ALLOD_KEYCHAIN_TESTS=1`).
- [ ] **Step 2: Verify failure** — `ALLOD_KEYCHAIN_TESTS=1 cargo test -p allod-core keychain` → FAIL (module missing).
- [ ] **Step 3: Implement** backend + chain default + migrate `--to keychain`.
- [ ] **Step 4: Run** `ALLOD_KEYCHAIN_TESTS=1 cargo test -p allod-core keychain` → PASS, then `cargo test --workspace` (without the env — keychain tests skip; ensures no accidental keychain dependency in the default path) → PASS. Also `cargo check --workspace --target x86_64-unknown-linux-gnu` if the target is installed; if not, note in the report that Linux compile relies on the cfg gates and CI will prove it.
- [ ] **Step 5: Manual checklist** (report only, do not block): create a real `allod`-service item via migrate on a scratch graph, observe the unlock prompt on sign, delete the item (`security delete-generic-password -s allod -a <acct>`).
- [ ] **Step 6: Commit**

```bash
git add crates/allod-core crates/allod/src/main.rs
git commit -m "feat(keys): macOS Keychain backend — store/resolve/sign with zeroized retrieval; migrate --to keychain"
```

---

### Task 6: wasm two-phase seam + git evaluation bindings

**Files:**
- Modify: `crates/allod-wasm/src/lib.rs`
- Modify: `crates/allod-graph/src/ops.rs` (unsigned changeset builder + signature attach, envelope payload builder split)
- Test: `crates/allod-wasm/tests/interop.test.ts` (extend)

**Interfaces:**
- Consumes: `flows::{decide_payload, decide_with_record}` (Task 3), `policy::{build_decision_record, attach_decider, decision_payload, evaluate_git, reviewers_unmet, GitChange}` (Task 3 + existing M2/M3 code), `Registry`, `State`.
- Produces (Rust, in `ops.rs`):

```rust
/// build_changeset minus the signing step: same changeset shape with
/// author.signature = "" placeholder, returning the hash that a signer
/// must sign. Refactor build_changeset to call this then attach.
pub fn build_changeset_unsigned(
    graph: &Graph, author_name: &str, intent: &str, ops: Vec<Value>,
) -> Result<(Value, String), AllodError>;

/// Set author.signature on a changeset produced by build_changeset_unsigned.
pub fn attach_changeset_signature(cs: &mut Value, signature: &str);

/// signed_envelope minus the signing step: (envelope-without-signature, payload).
pub fn envelope_payload_parts(author_name: &str, cs_hash: &str) -> Result<(Value, String), AllodError>;

/// Attach the signature to an envelope from envelope_payload_parts.
pub fn attach_envelope_signature(envelope: &mut Value, signature: &str);
```

(`build_changeset` and `signed_envelope` become thin wrappers: unsigned parts + `graph.signer(name)?.sign(payload)` + attach — the parity invariant falls out structurally. **Verify while implementing** that the changeset hash is computed over content that excludes `author.signature`; if the hash covers the signature field, the two-phase split must instead return the preimage before hashing — flag this to the coordinator if the assumption fails, do not improvise a format change.)

- Produces (wasm exports on `AllodGraph`; all JSON in/out via the existing `to_js`/`js_array_to_yaml_vec` helpers, structured errors via the existing `err` helper):

```text
// Native decide, two-phase
decide_payload(hash, verdict)            -> { record, payload }            (read-only)
decide_with_record(hash, record)         -> DecisionOutcome JSON           (mutating, persists)

// Commit, two-phase
commit_payload(author, intent, ops)      -> { changeset, hash }            (read-only)
commit_signed(changeset, signature, envelopes) -> Admission JSON           (mutating: attach sig, admit_or_hold, persist)

// Envelope, two-phase
envelope_payload(author, cs_hash)        -> { envelope, payload }          (read-only)
// (host attaches the signature itself: envelope.signature = sig — document
// this in the TS README section; no assembler export needed beyond passing
// the completed envelope into commit_signed / decide flows)

// Git evaluation (repo bytes supplied by the host; no git inside wasm)
git_checklist(repo, target_ref, ops)     -> { matched: [...], checklist: [...] }
    // ops: [[verb, path], ...]; binds policy::evaluate_git(policy, GitChange, Some((&state, &registry)))
git_satisfaction(subject, checklist, decisions) -> { unmet: [...] }
    // binds policy::reviewers_unmet; decisions = parsed note content array
git_decision_payload(subject, verdict)   -> { record, payload }
    // policy::build_decision_record with the graph's policy + ops::now_iso()
git_decision_attach(record, principal, signature) -> record
    // policy::attach_decider; pure, returns the completed signed record
```

`git_checklist` JSON shape: mirror whatever `evaluate_git`'s checklist type serializes to via serde — reuse the same serialization the CLI's `git eval` path prints from; the vitest test asserts parity with the CLI rather than a hand-specified shape.

- [ ] **Step 1: Write the failing vitest tests** in `interop.test.ts` (follow the file's existing fixture style — an in-memory graph via `new AllodGraph`, init, install-policy fixture). Cover:
  1. Two-phase native decide: create a held proposal (existing fixture code) → `decide_payload` → sign the payload in Node (`crypto.sign(null, Buffer.from(payload), keyObject)` with the owner's secret from the doc store, formatted `sig:ed25519:<hex>`) → `decide_with_record` → outcome `Admitted`. Assert the persisted evidence contains the attached record.
  2. Two-phase commit parity: `commit_payload` + Node-signed signature + `commit_signed([])` admits, and the resulting changeset in `log()` verifies via `verify()`.
  3. `git_checklist`: install a policy with a path-glob rule (reuse the governance/policy.yaml shapes from the repo fixture in `crates/allod-substrate-git` tests — inline the YAML in the test), pass `ops = [["M", "src/matched/file.rs"]]`, assert the rule matches and a reviewer requirement appears; a non-matching path yields an empty checklist.
  4. `git_satisfaction`: a checklist requiring role reviewer + empty decisions → unmet non-empty; with a decision record built via `git_decision_payload` → Node sign → `git_decision_attach` → satisfaction reports no unmet (fixture: bind the signing principal into the reviewer role in the policy).
  Node ed25519 helper (put at top of the test file):

```ts
import { createPrivateKey, sign as nodeSign } from "node:crypto";
function ed25519Sign(secretHex: string, payload: string): string {
  const der = Buffer.concat([
    Buffer.from("302e020100300506032b657004220420", "hex"),
    Buffer.from(secretHex, "hex"),
  ]);
  const key = createPrivateKey({ key: der, format: "der", type: "pkcs8" });
  const sig = nodeSign(null, Buffer.from(payload, "utf8"), key);
  return `sig:ed25519:${sig.toString("hex")}`;
}
```

- [ ] **Step 2: Run to verify failure** — `pnpm --dir crates/allod-wasm build && pnpm --dir crates/allod-wasm test` → new tests FAIL (exports missing).
- [ ] **Step 3: Implement** the ops.rs refactor (unsigned builders + attach; existing functions become wrappers), then the wasm exports. `decide_with_record` and `commit_signed` call `do_persist` like every mutating export.
- [ ] **Step 4: Run** `cargo test --workspace` AND the wasm suite → PASS.
- [ ] **Step 5: Commit**

```bash
git add crates/allod-graph/src/ops.rs crates/allod-wasm
git commit -m "feat(wasm): two-phase signing seam (decide/commit/envelope) + git_checklist/satisfaction/decision bindings"
```

---

### Task 7: Cross-backend parity vector suite

**Files:**
- Create: `crates/allod-core/tests/key_parity.rs`
- Test: itself

**Interfaces:**
- Consumes: `Keypair::from_secret_hex`, `FileBackend`, `Signer`, `KeychainBackend` (macOS, env-gated), `policy::{build_decision_record, attach_decider, decision_payload}`, `sign::verify`.

One suite, fixed inputs, every backend available in the environment must produce the identical signed record:

- [ ] **Step 1: Write the test**

```rust
// crates/allod-core/tests/key_parity.rs
use allod_core::keys::{FileBackend, Signer};
use allod_core::policy::{attach_decider, build_decision_record, decision_payload};
use allod_core::sign::{verify, Keypair};
use serde_yaml::Value;

const SECRET: &str = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
const TS: &str = "2026-08-04T12:00:00Z";

fn fixture_record() -> (Value, String) {
    let policy: Value = serde_yaml::from_str("rules: []").unwrap();
    let rec = build_decision_record(&policy, "git:deadbeef", "approve", TS).unwrap();
    let payload = decision_payload(&rec).unwrap();
    (rec, payload)
}

fn signed_via(signer: &Signer, rec: &Value, payload: &str) -> Value {
    let mut rec = rec.clone();
    attach_decider(&mut rec, signer.name(), &signer.sign(payload).unwrap());
    rec
}

#[test]
fn all_backends_produce_identical_records() {
    let (rec, payload) = fixture_record();
    let kp = Keypair::from_secret_hex("parity", SECRET).unwrap();
    let public = kp.public_hex();

    // Reference: in-memory keypair (what wasm-internal signing uses).
    let reference = signed_via(&Signer::local(Keypair::from_secret_hex("parity", SECRET).unwrap()), &rec, &payload);

    // File backend.
    let tmp = std::env::temp_dir().join(format!("allod-parity-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let be = FileBackend { create_dir: tmp.clone(), fallbacks: vec![] };
    let h = be.store("gid", &kp).unwrap();
    let via_file = signed_via(&Signer::from_backend(&be, h), &rec, &payload);
    assert_eq!(reference, via_file, "file backend must match in-memory signing byte for byte");

    // Keychain backend (macOS, opt-in).
    #[cfg(target_os = "macos")]
    if std::env::var("ALLOD_KEYCHAIN_TESTS").ok().as_deref() == Some("1") {
        use allod_core::keys_keychain::KeychainBackend;
        let kb = KeychainBackend { service: format!("allod-parity-{}", std::process::id()) };
        let kp2 = Keypair::from_secret_hex("parity", SECRET).unwrap();
        let h = kb.store("gid", &kp2).unwrap();
        let via_kc = signed_via(&Signer::from_backend(&kb, h), &rec, &payload);
        assert_eq!(reference, via_kc, "keychain backend must match");
        kb.delete("gid", "parity").unwrap();
    }

    // Every signature verifies against the public key.
    let sig = reference.get("deciders").unwrap().as_sequence().unwrap()[0]
        .get("signature").unwrap().as_str().unwrap();
    assert!(verify(&public, &payload, sig).is_ok());
    let _ = std::fs::remove_dir_all(&tmp);
}
```

- [ ] **Step 2: Run to verify it fails or passes for the right reason** — `cargo test -p allod-core --test key_parity` (it should PASS immediately if Tasks 1-5 are correct; if it fails, that is a real parity bug to fix in the backend, not in the test).
- [ ] **Step 3: Run once with keychain enabled** — `ALLOD_KEYCHAIN_TESTS=1 cargo test -p allod-core --test key_parity` → PASS.
- [ ] **Step 4: Full suite** — `cargo test --workspace` → PASS.
- [ ] **Step 5: Commit**

```bash
git add crates/allod-core/tests/key_parity.rs
git commit -m "test(keys): cross-backend parity vectors — file, keychain, in-memory signing byte-identical"
```

---

### Task 8: Dogfood — migrate the live repo key, docs, spec deviations

**Files:**
- Modify: `docs/superpowers/specs/2026-08-04-key-backends-design.md` (status + deviations section)
- Modify: `README.md` or `docs/` key-management section if one exists (check first; if none exists, add a short "Keys" section to the README covering `allod key where` / `migrate` and the storage locations)

**Steps:**

- [ ] **Step 1:** Build the release CLI in this worktree: `cargo build -q -p allod`.
- [ ] **Step 2:** NOTE for the coordinator (not the implementer): the live signing key lives ONLY in the primary checkout at `/Users/conner/code/allod/.allod/keys/conner.yaml` — the actual `allod key migrate` of the live key happens post-merge in the primary checkout, not in this worktree. The implementer instead verifies the migration path on a scratch graph: `allod init /tmp/kb-dogfood --owner o`, confirm the key landed under the XDG path (or `ALLOD_KEYS_DIR`), `allod key where`, then clean up `/tmp/kb-dogfood`.
- [ ] **Step 3:** Update the spec's Status line to "implemented (file + keychain); YubiKey PIV deferred" and add a Deviations subsection recording anything that diverged (e.g. ACL attachment capability, envelope assembler shape).
- [ ] **Step 4:** `cargo test --workspace` one final time; run `cargo clippy --workspace --all-targets` and fix new warnings in touched files.
- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "docs(keys): spec status + deviations; dogfood notes"
```

---

## Self-review notes (already applied)

- Spec coverage: trait + file backend (T1), resolution order/config + call-site migration + genesis XDG creation (T2), decision DRY groundwork the seam needs (T3), `key migrate`/init-gitignore (T4), keychain incl. `--to keychain` (T5), host-signing seam + freehold's three git bindings (T6), parity suite (T7), lifecycle/§6.2 needs no code (multiple active KeyRecords already legal — verified in spec), YubiKey + WebAuthn explicitly deferred.
- The wasm file-signing path "remains" (spec): untouched — existing `commit`/`decide` exports keep signing internally from in-store keys; the two-phase exports are additive.
- Type consistency: `Signer` method set (`name/sign/public_hex/key_id`) used identically in T2/T3/T6/T7; `graph_dir_component` shared by file and keychain accounts.
