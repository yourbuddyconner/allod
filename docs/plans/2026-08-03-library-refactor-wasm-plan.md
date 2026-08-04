# Allod Library Refactor + WASM Bindings Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Promote the functionality trapped in the `allod` CLI binary into library crates behind a storage abstraction, and publish a WASM npm package (`@allod/core`) so TypeScript clients can operate graphs in-process.

**Architecture:** `allod-core` gains a synchronous `DocStore` trait (documents keyed by relative path, mirroring today's `.allod/` file layout); `store::Graph` is rewired onto it. A new `allod-graph` crate hosts the operations API: a generic layer (changeset building over arbitrary types, propose/decide, registry introspection) plus the existing command flows rebuilt on it, returning typed values instead of printing. The CLI thins to parsing + formatting. A new `allod-wasm` crate wraps `allod-graph` via wasm-bindgen with JS persistence callbacks writing the same file layout.

**Tech Stack:** Rust 2021, serde_yaml, thiserror, wasm-bindgen + wasm-bindgen-futures + serde-wasm-bindgen, wasm-pack, vitest (Node ≥22) for the npm package tests.

## Global Constraints

- **No behavior changes.** `crates/allod/tests/mvp.rs` and `tests/demo.rs` assert on stdout substrings — every `println!` string the CLI emits today must be preserved byte-for-byte. Run `cargo test --workspace` after every task.
- **On-disk format invariant.** The `.allod/` layout documented in `crates/allod-core/src/store.rs:1-13` must not change. Appendix H vectors must regenerate byte-identical: `cargo run -p allod-vectors -- generate spec/vectors && git diff --exit-code spec/vectors`.
- **Schema-as-object materialization is out of scope** (sub-project 2). Do not touch how schema docs are stored or loaded beyond routing them through `DocStore`.
- Library code never prints. Only `crates/allod/src/main.rs` may call `println!`.
- New public items get doc comments in the existing style (see `store.rs` header).
- Commit after every green test run; commit messages in the repo's style (sentence case, imperative).

---

### Task 1: `DocStore` trait with `FsStore` and `MemStore`

**Files:**
- Create: `crates/allod-core/src/docstore.rs`
- Modify: `crates/allod-core/src/lib.rs` (add `pub mod docstore;` after line 13 `pub mod loader;`)

**Interfaces:**
- Produces (used by Task 2 and everything after):

```rust
/// Storage abstraction over the `.allod/` document tree. Paths are
/// relative to the `.allod/` root and always use `/` separators, e.g.
/// `"changesets/ab12.yaml"`, `"HEAD"`. Implementations are synchronous;
/// asynchronous hosts (the WASM bridge) persist around the trait.
pub trait DocStore: Send {
    /// Read a document; `Ok(None)` when absent.
    fn read(&self, path: &str) -> Result<Option<String>, String>;
    /// Write (create or replace) a document, creating parents.
    fn write(&self, path: &str, text: &str) -> Result<(), String>;
    /// Names (not paths) of documents directly under `dir`, sorted.
    fn list(&self, dir: &str) -> Result<Vec<String>, String>;
    /// Remove a document; removing an absent document is Ok.
    fn remove(&self, path: &str) -> Result<(), String>;
}

pub struct FsStore { /* root: PathBuf — the `.allod/` directory */ }
impl FsStore {
    /// `dir` is the graph directory; the store roots at `dir/.allod`.
    pub fn create(dir: &Path) -> Result<FsStore, String>;  // mkdir -p root
    pub fn open(dir: &Path) -> Result<FsStore, String>;    // no existence check beyond root
}

pub struct MemStore { /* Mutex<BTreeMap<String, String>> */ }
impl MemStore {
    pub fn new() -> MemStore;
    /// Every (path, text) pair, sorted by path — the persistence dump.
    pub fn dump(&self) -> Vec<(String, String)>;
    /// Bulk-load pairs (used to hydrate from persisted state).
    pub fn load(&self, docs: Vec<(String, String)>);
}
```

`write`/`remove` take `&self`: `FsStore` is stateless over the filesystem, `MemStore` uses interior mutability (`Mutex`), so `store::Graph` can keep its `&self` methods in Task 2.

- [ ] **Step 1: Write the failing conformance test** at the bottom of `docstore.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn conformance(store: &dyn DocStore) {
        assert_eq!(store.read("HEAD").unwrap(), None);
        store.write("HEAD", "sha256:abc").unwrap();
        assert_eq!(store.read("HEAD").unwrap().as_deref(), Some("sha256:abc"));
        store.write("changesets/b.yaml", "b: 1\n").unwrap();
        store.write("changesets/a.yaml", "a: 1\n").unwrap();
        assert_eq!(
            store.list("changesets").unwrap(),
            vec!["a.yaml".to_string(), "b.yaml".to_string()]
        );
        assert_eq!(store.list("proposals").unwrap(), Vec::<String>::new());
        store.remove("changesets/a.yaml").unwrap();
        store.remove("changesets/a.yaml").unwrap(); // idempotent
        assert_eq!(store.list("changesets").unwrap(), vec!["b.yaml".to_string()]);
    }

    #[test]
    fn memstore_conforms() {
        conformance(&MemStore::new());
    }

    #[test]
    fn fsstore_conforms() {
        let dir = std::env::temp_dir().join(format!("allod-docstore-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = FsStore::create(&dir).unwrap();
        conformance(&store);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn memstore_dump_load_round_trips() {
        let a = MemStore::new();
        a.write("HEAD", "h").unwrap();
        a.write("keys/o.yaml", "k: 1\n").unwrap();
        let b = MemStore::new();
        b.load(a.dump());
        assert_eq!(b.dump(), a.dump());
    }
}
```

- [ ] **Step 2:** `cargo test -p allod-core docstore` — expect compile failure (types missing).
- [ ] **Step 3:** Implement the trait and both stores. `FsStore::list` must skip subdirectories and return sorted file names. `MemStore::list` filters keys by `dir/` prefix with no further `/`.
- [ ] **Step 4:** `cargo test -p allod-core` — all pass.
- [ ] **Step 5:** Commit: `Add the DocStore trait with filesystem and in-memory implementations`

---

### Task 2: Rewire `store::Graph` onto `DocStore`

**Files:**
- Modify: `crates/allod-core/src/store.rs` (whole file)
- Modify: `crates/allod/src/main.rs:661-667` (cmd_envelope's raw evidence write)

**Interfaces:**
- Consumes: `DocStore`, `FsStore`, `MemStore` from Task 1.
- Produces:

```rust
pub struct Graph {
    pub dir: PathBuf,            // kept: md.rs writes bundles relative to it
    store: Box<dyn DocStore>,
}
impl Graph {
    pub fn create(dir: &Path) -> Result<Graph, String>;      // FsStore::create, unchanged signature
    pub fn open(dir: &Path) -> Result<Graph, String>;        // FsStore::open + graph.yaml existence check, unchanged signature
    pub fn with_store(store: Box<dyn DocStore>) -> Graph;    // dir = PathBuf::new(); WASM path
    pub fn open_with_store(store: Box<dyn DocStore>) -> Result<Graph, String>; // checks graph.yaml
    /// New: replaces the raw write in cmd_envelope.
    pub fn write_evidence(&self, hash: &str, evidence: &Value) -> Result<(), String>;
    // every existing method keeps its exact signature
}
```

- [ ] **Step 1:** Write a failing test in `store.rs` proving `Graph` works over `MemStore`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::docstore::MemStore;

    #[test]
    fn graph_over_memstore() {
        let graph = Graph::with_store(Box::new(MemStore::new()));
        graph.write_meta("sha256:genesis", &["principal:o".into()]).unwrap();
        assert_eq!(graph.roots().unwrap(), vec!["principal:o".to_string()]);
        assert_eq!(graph.head().unwrap(), None);
        let cs: Value = serde_yaml::from_str("hash: sha256:ab\nparents: []").unwrap();
        graph.append_changeset(&cs, "sha256:ab", None).unwrap();
        assert_eq!(graph.head().unwrap().as_deref(), Some("sha256:ab"));
        assert!(graph.read_evidence("sha256:ab").unwrap().is_none());
        graph.write_evidence("sha256:ab", &cs).unwrap();
        assert!(graph.read_evidence("sha256:ab").unwrap().is_some());
    }
}
```

- [ ] **Step 2:** `cargo test -p allod-core store` — fails (no `with_store`).
- [ ] **Step 3:** Convert every method: `read_yaml(path)`/`write_yaml(path, doc)` become store reads/writes of relative paths (`"graph.yaml"`, `"schema/{name}.yaml"`, `"keys/{name}.yaml"`, `"changesets/{short}.yaml"`, `"changesets/{short}.evidence.yaml"`, `"proposals/…"`, `"checkpoints/{short}.yaml"`, `"HEAD"`). `registry()` (store.rs:179-186) currently calls `crate::load_dir` on the schema directory — replace with loading from `schema_docs()` text via the existing loader entry point for in-memory docs (add `load_docs(docs: &[(String, Value)])` to `loader.rs` if only a dir-based entry exists; it must produce an identical `Registry` — check `loader.rs` first, reuse whatever `load_dir` does after reading files). Absent-file behaviors must be preserved exactly: `head()` → `Ok(None)`, `read_evidence` → `Ok(None)`, `read_proposal_evidence` → empty decisions/envelopes doc, `checkpoints()` on missing dir → `Ok(vec![])`, `open` on missing `graph.yaml` → the exact error string `"{dir} is not an allod graph (no .allod/graph.yaml)"`.
- [ ] **Step 4:** Replace `crates/allod/src/main.rs:661-667` (raw `std::fs::write` of the evidence file) with `graph.write_evidence(cs_hash, &evidence_file)?;`.
- [ ] **Step 5:** `cargo test --workspace` — everything passes, demos untouched.
- [ ] **Step 6:** Regenerate vectors, confirm no diff: `cargo run -p allod-vectors -- generate spec/vectors && git diff --exit-code spec/vectors`.
- [ ] **Step 7:** Commit: `Rewire graph storage through DocStore`

---

### Task 3: `allod-graph` crate — errors, admission, generic operations

**Files:**
- Create: `crates/allod-graph/Cargo.toml` (deps: `allod-core = { path = "../allod-core" }`, `serde = "1"`, `serde_yaml = "0.9"`, `thiserror = "1"`, `rand = "0.8"`)
- Create: `crates/allod-graph/src/lib.rs`, `error.rs`, `ops.rs`
- Modify: root `Cargo.toml` workspace members

**Interfaces:**
- Produces (the generic layer everything else builds on):

```rust
// error.rs
#[derive(Debug, thiserror::Error)]
pub enum AllodError {
    #[error("schema violation: {0}")]  SchemaViolation(String),
    #[error("policy rejected: {0}")]   PolicyRejected(String),
    #[error("hash mismatch: {0}")]     HashMismatch(String),
    #[error("invalid signature: {0}")] SignatureInvalid(String),
    #[error("unknown principal: {0}")] UnknownPrincipal(String),
    #[error("not found: {0}")]         NotFound(String),
    #[error("storage: {0}")]           Storage(String),
    #[error("{0}")]                    Other(String),
}
impl From<String> for AllodError { /* Other(s) — legacy boundary */ }

// ops.rs — moved from main.rs with pub visibility, printing removed:
pub fn uuid4() -> String;                         // main.rs:41
pub fn now_iso() -> String;                       // main.rs:58
pub fn short(hash: &str) -> String;               // main.rs:81
pub fn build_changeset(graph: &Graph, author: &Keypair, intent: &str, ops: Vec<Value>)
    -> Result<(Value, String), AllodError>;       // main.rs:88-114 verbatim
pub fn key_record(kp: &Keypair) -> Value;         // main.rs:116
pub fn evidence_doc(decisions: &[Value], envelopes: &[Value]) -> Value; // main.rs:125

/// Draft builders producing operation Values (the shapes main.rs builds inline):
pub fn create_node_op(id: &str, type_ref: &str, attributes: Value, provenance: Option<Value>) -> Value;
pub fn create_edge_op(id: &str, type_ref: &str, from: &str, to: &str, attributes: Option<Value>) -> Value;
pub fn classification_op(subject: &str, term: &str, asserted_by: &str, basis: &str) -> Value; // generalizes main.rs:342
pub fn update_node_op(id: &str, prior_rev: &str, attributes: Value) -> Value;
pub fn delete_op(kind: &str, id: &str) -> Value;

/// The admission outcome. Held is Ok — the system working as intended.
#[derive(Debug, serde::Serialize)]
pub enum Admission {
    Admitted { hash: String, matched_rules: Vec<String> },
    Held { hash: String, checklist: ChecklistView },
}
#[derive(Debug, serde::Serialize)]
pub struct ChecklistView {
    pub matched_rules: Vec<String>,
    pub reviewers: Vec<(String, u32)>,
    pub attestations: Vec<String>,
    pub root_required: bool,
}

/// admit_or_hold (main.rs:154-200) minus printing, returning Admission.
pub fn admit_or_hold(graph: &Graph, author_name: &str, cs: &Value, hash: &str,
    envelopes: Vec<Value>) -> Result<Admission, AllodError>;

/// One call for "build ops, sign, submit": the generic write path.
pub fn commit(graph: &Graph, author_name: &str, intent: &str, ops: Vec<Value>,
    envelopes: Vec<Value>) -> Result<Admission, AllodError>;
```

`ChecklistView` is built from `allod_core::policy::Checklist` (fields observed in `print_checklist`, main.rs:132-152: `matched_rules`, `reviewers`, `attestations`, `root_required`).

- [ ] **Step 1:** Scaffold the crate; add to workspace; `cargo build -p allod-graph`.
- [ ] **Step 2:** Write the failing test `crates/allod-graph/tests/ops.rs` — full memory loop over `MemStore` without any CLI:

```rust
use allod_core::docstore::MemStore;
use allod_core::store::Graph;
use allod_graph::ops::{self, Admission};

// Helper: genesis via the flows layer arrives in Task 4; here, build the
// smallest valid graph by hand exactly as cmd_init_profile does, using a
// schema_dir pointing at the repo's ontologies/ (native test only).
mod common;  // create tests/common/mod.rs with fn init_memory_graph() -> Graph
             // (port of main.rs:210-286 against ontologies/, owner "o")

#[test]
fn scratch_note_admits_and_preference_holds() {
    let graph = common::init_memory_graph();
    // agent registration (port of main.rs:288-328 for kind "agent", by "o")
    common::add_agent(&graph, "jarvis", "o");

    let note_id = ops::uuid4();
    let note_ops = vec![
        ops::create_node_op(&note_id, "memory/Note@1",
            serde_yaml::from_str("content: prefers tea").unwrap(),
            Some(common::provenance("jarvis"))),
        ops::classification_op(&format!("node:{note_id}"), "workspace/scratch@1",
            "principal:jarvis", "model-assisted"),
    ];
    match ops::commit(&graph, "jarvis", "Scratch note", note_ops, vec![]).unwrap() {
        Admission::Admitted { .. } => {}
        other => panic!("scratch should admit, got {other:?}"),
    }

    let pref_id = ops::uuid4();
    let pref_ops = vec![
        ops::create_node_op(&pref_id, "memory/Preference@1",
            serde_yaml::from_str("statement: tea over coffee\nstrength: strong").unwrap(),
            Some(common::provenance("jarvis"))),
        ops::classification_op(&format!("node:{pref_id}"), "work@1",
            "principal:jarvis", "model-assisted"),
    ];
    match ops::commit(&graph, "jarvis", "Propose preference", pref_ops, vec![]).unwrap() {
        Admission::Held { checklist, .. } => {
            assert!(!checklist.matched_rules.is_empty());
        }
        other => panic!("preference should hold, got {other:?}"),
    }
}
```

(`common::init_memory_graph` reads ontology files from `env!("CARGO_MANIFEST_DIR")/../../ontologies` — native tests may use the fs to *read source YAML*; the graph itself lives on `MemStore`.)

- [ ] **Step 3:** Run: `cargo test -p allod-graph` — fails.
- [ ] **Step 4:** Implement `error.rs` and `ops.rs` by moving code from `main.rs` (line refs above), converting `Result<_, String>` at this layer and stripping every `println!`. `main.rs` keeps compiling by importing the moved helpers from `allod_graph::ops` (add the dependency to `crates/allod/Cargo.toml`); its own `admit_or_hold` wrapper now calls the library and reprints from the returned `Admission` — the exact strings `"  ✓ admitted {} ({basis})"`, `"  ⧗ held as proposal {}"`, and `print_checklist` output, unchanged.
- [ ] **Step 5:** `cargo test --workspace` — new test and mvp.rs both green.
- [ ] **Step 6:** Commit: `Add allod-graph: structured errors, admission, and the generic operations layer`

---

### Task 4: Move the command flows into `allod-graph::flows`

**Files:**
- Create: `crates/allod-graph/src/flows.rs`
- Modify: `crates/allod/src/main.rs` (each `cmd_*` becomes parse → call → print)

**Interfaces:**
- Consumes: Task 3's `ops`, `Admission`, `AllodError`.
- Produces (each is the body of the corresponding `cmd_*`, printing removed, typed result added — source line refs from `main.rs`):

```rust
pub struct ProfileSource { pub name: String, pub docs: Vec<(String, Value)>, pub policy: Value }
/// Resolve a named profile from a schema directory (native path; the
/// embedded variant arrives in Task 8). Ports main.rs:226-256.
pub fn profile_from_dir(profile: &str, schema_dir: &Path) -> Result<ProfileSource, AllodError>;

pub struct InitResult { pub graph_id: String, pub owner: String }
pub fn init(graph: &Graph, owner: &str, profile: ProfileSource) -> Result<InitResult, AllodError>; // main.rs:210-286

pub struct PrincipalAdded { pub node_id: String, pub admission: Admission }
pub fn principal_add(graph: &Graph, name: &str, kind: &str, by: &str) -> Result<PrincipalAdded, AllodError>; // main.rs:288-328

pub struct NoteResult { pub note_id: String, pub admission: Admission }
pub fn note(graph: &Graph, agent: &str, content: &str) -> Result<NoteResult, AllodError>; // main.rs:355-377

pub struct ProposalResult { pub hash: String, pub admission: Admission }
pub fn propose_preference(graph: &Graph, agent: &str, statement: &str, strength: &str,
    from_note: Option<&str>) -> Result<ProposalResult, AllodError>; // main.rs:379-440

#[derive(serde::Serialize)]
pub enum DecisionOutcome {
    Rejected,
    StillUnmet { unmet: Vec<String> },
    Admitted { degraded: Vec<String> },
}
pub fn decide(graph: &Graph, hash: &str, by: &str, verdict: &str) -> Result<DecisionOutcome, AllodError>; // main.rs:450-524

pub fn classify(graph: &Graph, node_id: &str, term: &str, by: &str, basis: &str)
    -> Result<Admission, AllodError>;                                // main.rs:526-552
pub struct CheckpointResult { pub revision: String, pub state_hash: String }
pub fn checkpoint(graph: &Graph, by: &str) -> Result<CheckpointResult, AllodError>; // main.rs:554-582
pub fn trust(graph: &Graph, measurement: &str) -> Result<(), AllodError>;           // main.rs:595-600
pub enum EnvelopeOutcome { Verified(String), Degraded(String) }
pub fn envelope(graph: &Graph, cs_hash: &str, by: &str, tool: &str)
    -> Result<EnvelopeOutcome, AllodError>;                          // main.rs:605-669

#[derive(serde::Serialize)]
pub struct ProposalSummary { pub hash: String, pub intent: String, pub author: String }
pub fn proposals(graph: &Graph) -> Result<Vec<ProposalSummary>, AllodError>;        // main.rs:671-687
#[derive(serde::Serialize)]
pub struct ChangesetSummary { pub hash: String, pub author: String, pub op_count: usize, pub intent: String }
pub fn log(graph: &Graph) -> Result<Vec<ChangesetSummary>, AllodError>;             // main.rs:689-708
#[derive(serde::Serialize)]
pub struct EntitySummary { pub type_ref: String, pub label: String, pub derived_by: Option<String> }
#[derive(serde::Serialize)]
pub struct StateView { pub state_hash: String, pub nodes: Vec<EntitySummary> }
pub fn state(graph: &Graph) -> Result<StateView, AllodError>;                       // main.rs:710-737
#[derive(serde::Serialize)]
pub struct VerifyReport { /* one entry per changeset per level, plus overall */ }
pub fn verify(graph: &Graph) -> Result<VerifyReport, AllodError>;                   // main.rs:739-859
```

For `VerifyReport`: read `main.rs:739-859` before shaping it — it must carry, per changeset: hash, author, the three per-level results (integrity/authorship/governance) each `verified` | `degraded(reason)` | `failed(reason)`, and `admitted_by`; plus overall `ok` and the degraded list, so the CLI can reproduce its current output exactly and Freehold gets the whole story as data.

- [ ] **Step 1:** For each flow, in this order — `init`, `principal_add`, `note`, `propose_preference`, `decide`, `classify`, `checkpoint`, `trust`, `envelope`, `proposals`, `log`, `state`, `verify` — do the move as its own red/green cycle: write a small library test in `crates/allod-graph/tests/flows.rs` exercising the flow over `MemStore` (reuse `common::init_memory_graph`), watch it fail, move the body from `main.rs`, replace the `cmd_*` with parse → `flows::x(...)` → print (strings identical to today, including `"  ✓ graph {} (owner {owner})"`, `"  ✗ rejected {} — proposal and decision record stay auditable"`, `"  ⧗ decision recorded; still unmet:"`, `"  ✓ approved and admitted {}"`, `"      degraded: {note}"`, the proposals/log/show formats at main.rs:671-737, and the whole verify output), then run `cargo test --workspace`.
- [ ] **Step 2:** After all flows: `main.rs` contains no domain logic — grep it for `build_changeset|apply_changeset|policy::` and expect only imports and the demo functions (demos may keep orchestrating flows).
- [ ] **Step 3:** Vectors check (no diff), `cargo test --workspace`.
- [ ] **Step 4:** Commit per flow or in coherent groups: `Move <flow> into allod-graph`

---

### Task 5: Move the markdown bundle (`md.rs`)

**Files:**
- Create: `crates/allod-graph/src/md.rs` (from `crates/allod/src/md.rs`, whole module)
- Modify: `crates/allod/src/main.rs` (md command arms), delete `crates/allod/src/md.rs`

**Interfaces:**
- Produces: `pub fn export(graph: &Graph, out: &Path) -> Result<ExportReport, AllodError>` and `pub fn import(graph: &Graph, bundle: &Path, as_principal: &str) -> Result<ImportReport, AllodError>` where `ExportReport { files: usize, state_hash: String }` and `ImportReport { admissions: Vec<Admission>, skipped: Vec<(PathBuf, String)> }` — shape them from what the current functions print (read `md.rs` fully first; keep the bundle output format byte-identical).
- Note: bundle I/O (writing the export directory) is real filesystem work and stays `std::fs` — it is the *bundle*, not the graph store. Signature change from `graph_dir: &Path` to `graph: &Graph` (callers open the graph).

- [ ] **Step 1:** Failing test in `crates/allod-graph/tests/md.rs`: init a MemStore graph via `common`, add one admitted note, `export` to a temp dir, assert `manifest.yaml` exists and `ExportReport.state_hash` equals `graph.fold()?.state_hash()?`; re-`import` of the unmodified bundle produces zero new admissions (round-trip, §7.4).
- [ ] **Step 2:** Move, adapt signatures, keep every emitted file byte-identical (mvp.rs step 6 depends on it).
- [ ] **Step 3:** `cargo test --workspace`; commit: `Move the markdown bundle into allod-graph`

---

### Task 6: Move federation (`fed.rs`)

**Files:**
- Create: `crates/allod-graph/src/fed.rs` (from `crates/allod/src/fed.rs`)
- Modify: `crates/allod/src/main.rs` (fed command arms), delete `crates/allod/src/fed.rs`

**Interfaces:**
- Produces, signatures adapted from the current `pub fn`s (fed.rs:28-413): `peer_add`, `grant` (returns the grant id), `revoke`, `bundle` — which currently writes the bundle to an `out: &Path`; split it into `pub fn make_bundle(graph: &Graph, grant_id: &str, by: &str) -> Result<Value, AllodError>` (pure data — the WASM surface) plus a thin fs writer used by the CLI — and `import(graph, bundle: &Value, as_principal) -> Result<Vec<Admission>, AllodError>`.

- [ ] **Step 1:** Failing test in `crates/allod-graph/tests/fed.rs`: two MemStore graphs; A grants a region to B, `make_bundle`, B imports under its policy; assert the imported object's lineage carries the `allod:` reference to A's changeset (grep fed.rs for the lineage insertion to name the exact field); revoke, assert a second `make_bundle` fails with a typed error.
- [ ] **Step 2:** Move + adapt; CLI arms reprint current strings; `demo-federation` must pass unchanged.
- [ ] **Step 3:** `cargo test --workspace`; commit: `Move federation into allod-graph`

---

### Task 7: Move repo import (`repo.rs`) behind the `native` feature

**Files:**
- Create: `crates/allod-graph/src/repo.rs` (from `crates/allod/src/repo.rs`)
- Modify: `crates/allod-graph/Cargo.toml` (`[features] native = []`, default on; `repo` module `#[cfg(feature = "native")]`), `crates/allod/src/main.rs`, delete `crates/allod/src/repo.rs`

- [ ] **Step 1:** Move `import_commit`, `semantic_diff`, `make_sample_repo` (repo.rs:260, 496, 616) with typed results in place of prints (`semantic_diff` returns the diff structure it currently renders; read the function to shape it). Everything that spawns `git` sits under `#[cfg(feature = "native")]`.
- [ ] **Step 2:** `cargo test --workspace` (demo-code passes) and `cargo check -p allod-graph --no-default-features` (compiles without repo/git).
- [ ] **Step 3:** Commit: `Move repo import into allod-graph behind the native feature`

---

### Task 8: Registry introspection + embedded reference ontologies

**Files:**
- Create: `crates/allod-graph/src/schema.rs`, `crates/allod-graph/src/profiles.rs`
- Modify: `crates/allod-graph/src/lib.rs`

**Interfaces:**
- Produces:

```rust
// schema.rs — read from the graph's Registry (allod_core::Registry):
#[derive(serde::Serialize)]
pub struct SchemaDescription {
    pub entity_types: Vec<EntityTypeView>,   // name, version, extends, attributes: Vec<AttributeView{name, type_expr, required}>
    pub edge_types: Vec<EdgeTypeView>,       // name, version, domain, range, cardinality
    pub terms: Vec<TermView>,                // name, version, parents, status
}
pub fn describe(graph: &Graph) -> Result<SchemaDescription, AllodError>;

// profiles.rs — include_str! the repo's ontology YAML at compile time:
pub fn embedded_profile(name: &str) -> Result<ProfileSource, AllodError>; // "memory" | "code"
```

`embedded_profile` embeds exactly the files `profile_from_dir` reads (main.rs:226-243: core/ontology, memory/ontology, memory/taxonomy, memory/policy-local; code equivalents) via `include_str!("../../../ontologies/…")`. `flows::init` gains nothing — callers choose `profile_from_dir` (CLI `--schema-dir`) or `embedded_profile` (WASM default).

- [ ] **Step 1:** Failing tests: `describe` on `common::init_memory_graph()` lists `memory/Note` with a `content` attribute and the `workspace/scratch` term with parent `workspace`; `embedded_profile("memory")` equals `profile_from_dir("memory", <repo ontologies>)` document-for-document.
- [ ] **Step 2:** Implement by walking `Registry` (read `registry.rs` for its shape — `Package`, `Taxonomy` are exported from `allod_core`).
- [ ] **Step 3:** `cargo test --workspace`; commit: `Add registry introspection and embedded reference profiles`

---

### Task 9: `allod-wasm` → npm `@allod/core`

**Files:**
- Create: `crates/allod-wasm/Cargo.toml`, `crates/allod-wasm/src/lib.rs`
- Create: `crates/allod-wasm/js/store.ts` (Node fs persistence backend), `crates/allod-wasm/js/index.ts` (package entry), `crates/allod-wasm/package.json`, `crates/allod-wasm/tests/memory-flow.test.ts`
- Modify: root `Cargo.toml` (workspace member)

**Interfaces:**

```toml
# Cargo.toml essentials
[lib] crate-type = ["cdylib", "rlib"]
[dependencies]
allod-graph = { path = "../allod-graph", default-features = false }
allod-core  = { path = "../allod-core" }
wasm-bindgen = "0.2"
wasm-bindgen-futures = "0.4"
serde-wasm-bindgen = "0.6"
getrandom = { version = "0.2", features = ["js"] }   # rand/ed25519 in wasm
```

```rust
// lib.rs — one exported class wrapping MemStore + allod-graph:
#[wasm_bindgen]
pub struct AllodGraph { /* graph: Graph over MemStore, persist: js_sys::Function */ }

#[wasm_bindgen]
impl AllodGraph {
    /// docs: Array<[path, text]> hydrating MemStore (empty for a new graph).
    /// persist: async (dump: Array<[path, text]>) => void — called and awaited
    /// after every mutating call, BEFORE that call resolves.
    #[wasm_bindgen(constructor)]
    pub fn new(docs: JsValue, persist: js_sys::Function) -> Result<AllodGraph, JsValue>;

    pub async fn init(&mut self, owner: String, profile: String) -> Result<JsValue, JsValue>;   // embedded_profile
    pub async fn principal_add(&mut self, name: String, kind: String, by: String) -> Result<JsValue, JsValue>;
    pub async fn commit(&mut self, author: String, intent: String, ops: JsValue, envelopes: JsValue) -> Result<JsValue, JsValue>; // generic layer
    pub async fn note(&mut self, agent: String, content: String) -> Result<JsValue, JsValue>;
    pub async fn propose_preference(&mut self, agent: String, statement: String, strength: String, from_note: Option<String>) -> Result<JsValue, JsValue>;
    pub async fn decide(&mut self, hash: String, by: String, verdict: String) -> Result<JsValue, JsValue>;
    pub async fn classify(&mut self, node_id: String, term: String, by: String, basis: String) -> Result<JsValue, JsValue>;
    pub async fn install_schema(&mut self, name: String, doc_yaml: String) -> Result<(), JsValue>;
    pub fn proposals(&self) -> Result<JsValue, JsValue>;
    pub fn log(&self) -> Result<JsValue, JsValue>;
    pub fn state(&self) -> Result<JsValue, JsValue>;
    pub fn verify(&self) -> Result<JsValue, JsValue>;
    pub fn describe_schema(&self) -> Result<JsValue, JsValue>;
    pub fn export_md(&self) -> Result<JsValue, JsValue>;  // returns Array<[relpath, text]> — no fs in wasm
}
```

All results serialize through `serde-wasm-bindgen` from the typed structs of Tasks 3/4/8. Errors become `JsValue` strings carrying the `AllodError` display. `md::export`'s wasm variant returns file pairs instead of writing (add `md::export_docs(graph) -> Vec<(String, String)>` in Task 5's module, used by both paths). Repo import is absent (`default-features = false`).

```json
// package.json essentials
{ "name": "@allod/core", "type": "module",
  "scripts": { "build": "wasm-pack build --target nodejs --out-dir pkg", "test": "vitest run" } }
```

```ts
// js/store.ts — the Node persistence backend:
export function fsBackend(graphDir: string) {
  // load(): read every file under graphDir/.allod into Array<[relpath, text]>
  // persist(dump): write each pair to graphDir/.allod/<relpath>, mkdir -p,
  //   then prune files not in dump (removed proposals). fs/promises.
}
```

- [ ] **Step 1:** Write `tests/memory-flow.test.ts` (failing — package not built):

```ts
import { describe, expect, test } from "vitest";
import { mkdtempSync } from "node:fs"; import { tmpdir } from "node:os"; import { join } from "node:path";
import { AllodGraph } from "../pkg/allod_wasm.js";
import { fsBackend } from "../js/store.js";

test("the founding loop, from TypeScript", async () => {
  const dir = mkdtempSync(join(tmpdir(), "allod-wasm-"));
  const backend = fsBackend(dir);
  const g = new AllodGraph([], backend.persist);
  await g.init("conner", "memory");
  await g.principal_add("jarvis", "agent", "conner");
  const note = await g.note("jarvis", "prefers tea");
  expect(note.admission.Admitted).toBeDefined();
  const pref = await g.propose_preference("jarvis", "tea over coffee", "strong", note.note_id);
  expect(pref.admission.Held).toBeDefined();
  const decided = await g.decide(pref.hash, "conner", "approve");
  expect(decided.Admitted).toBeDefined();
  const report = g.verify();
  expect(report.ok).toBe(true);
  // resume from persisted state: a second instance reads the same dir
  const g2 = new AllodGraph(backend.load(), backend.persist);
  expect(g2.state().state_hash).toEqual(g.state().state_hash);
});
```

- [ ] **Step 2:** Implement `lib.rs` + `js/`; `pnpm wasm-pack build`; `pnpm vitest run` — green.
- [ ] **Step 3:** Commit: `Add allod-wasm: @allod/core with a Node persistence backend`

---

### Task 10: Interop tests and CI

**Files:**
- Create: `crates/allod-wasm/tests/interop.test.ts`
- Modify: `.github/workflows/*` (or create `.github/workflows/wasm.yml` if CI lives elsewhere — check the repo's existing CI first and follow its shape)

- [ ] **Step 1:** Interop test (exit criterion 4): the TS suite writes a graph via `fsBackend`, then the test spawns the Rust CLI — `cargo run -p allod -- verify <dir>` (build once in CI beforehand) — and asserts exit 0 and stdout containing the verify success marker; the reverse direction runs `cargo run -p allod -- init/note/...` into a temp dir, then opens it with `new AllodGraph(backend.load(), …)` and asserts `verify().ok`.
- [ ] **Step 2:** CI: a job with Rust + Node ≥22 + wasm-pack that runs `cargo test --workspace`, the vectors no-diff check, `wasm-pack build`, and `vitest run` including interop.
- [ ] **Step 3:** Full pass locally; commit: `Add Rust–TypeScript interop tests and the wasm CI job`

---

## Self-review notes (already applied)

- Spec exit criterion 1 (workspace tests + vectors) — Tasks 2–7 each end with it. Criterion 2 (demos unchanged) — Tasks 4/6/7. Criterion 3 (TS memory flow) — Task 9. Criterion 4 (interop both directions) — Task 10.
- The `Held`-is-`Ok` decision (spec decision 3) is encoded in `Admission` (Task 3), consumed by Tasks 4/9.
- The persistence contract (spec decision 4: awaited before resolve, same file layout) is Task 9's constructor contract + `fsBackend`, proven by the resume assertion and the interop test.
- Names used across tasks were cross-checked: `Admission`, `ChecklistView`, `commit`, `flows::*` signatures, `embedded_profile`, `export_docs`.
