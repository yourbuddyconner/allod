# Substrate Refactor (Milestone 1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Introduce the §3.1 substrate abstraction as a new `allod-substrate` crate with a `NativeSubstrate` adapter over the existing changeset log, a conformance suite, and one real consumer (`flows::verify` integrity/authorship) rewired through it — with zero behavior change.

**Architecture:** The codebase is `serde_yaml::Value`-based with `Result<_, String>` errors; the trait follows that idiom rather than introducing typed changesets. `allod-substrate` depends only on `allod-core`. `allod-graph` gains a dependency on `allod-substrate` (no cycle: `allod-substrate` never depends on `allod-graph`; `allod-graph` appears only as a dev-dependency of `allod-substrate` for fixture tests). The git implementation of the same trait is milestone 2, in a separate crate.

**Tech Stack:** Rust 2021 workspace, serde_yaml, existing `allod-core` (store/fold/policy/sign). No new external dependencies.

## Global Constraints

- Zero behavior change: every existing test in the workspace passes unmodified. Run `cargo test --workspace` after each task.
- Error idiom: `Result<T, String>` (matches `allod-core`); `allod-graph` call sites may wrap in `AllodError::Other`.
- `allod-substrate` must stay wasm-safe: no filesystem, no process spawning, no new deps beyond `serde_yaml` and `allod-core`.
- Hash strings always carry the algorithm prefix (`sha256:…`, §1.7).
- Spec references in doc comments cite section numbers the way existing crates do (e.g. `(§3.1)`).
- Worktree: all work happens in the current worktree (`.claude/worktrees/governed-review`), branch `worktree-governed-review`.

---

### Task 1: `allod-substrate` crate with the §3.1 trait and core types

**Files:**
- Create: `crates/allod-substrate/Cargo.toml`
- Create: `crates/allod-substrate/src/lib.rs`
- Modify: `Cargo.toml` (workspace members list, currently `members = ["crates/allod", "crates/allod-core", "crates/allod-graph", "crates/allod-lint", "crates/allod-vectors", "crates/allod-wasm"]`)

**Interfaces:**
- Consumes: `serde_yaml::Value` only.
- Produces (later tasks rely on these exact names):
  - `pub type RevHash = String;` `pub type RefName = String;`
  - `pub struct Revision { pub hash: RevHash, pub parents: Vec<RevHash>, pub author: serde_yaml::Value, pub timestamp: Option<String>, pub signed: bool }`
  - `pub enum AuthorVerdict { Verified { principal: String, key_id: String }, Unsigned, Failed(String) }`
  - `pub trait Substrate { fn revision(&self, hash: &str) -> Result<Revision, String>; fn operation_set(&self, hash: &str) -> Result<Vec<serde_yaml::Value>, String>; fn state_hash(&self, hash: &str) -> Result<String, String>; fn heads(&self) -> Result<Vec<(RefName, RevHash)>, String>; fn verify_authorship(&self, hash: &str) -> Result<AuthorVerdict, String>; }`

- [ ] **Step 1: Write the failing test**

In `crates/allod-substrate/src/lib.rs` (test module at the bottom; the types don't exist yet, so this file won't compile until Step 3 — that is the failing state):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // The trait must be object-safe: consumers hold `&dyn Substrate`.
    fn takes_dyn(_s: &dyn Substrate) {}

    struct Empty;
    impl Substrate for Empty {
        fn revision(&self, _hash: &str) -> Result<Revision, String> {
            Err("empty".into())
        }
        fn operation_set(&self, _hash: &str) -> Result<Vec<serde_yaml::Value>, String> {
            Err("empty".into())
        }
        fn state_hash(&self, _hash: &str) -> Result<String, String> {
            Err("empty".into())
        }
        fn heads(&self) -> Result<Vec<(RefName, RevHash)>, String> {
            Ok(vec![])
        }
        fn verify_authorship(&self, _hash: &str) -> Result<AuthorVerdict, String> {
            Ok(AuthorVerdict::Unsigned)
        }
    }

    #[test]
    fn trait_is_object_safe_and_types_construct() {
        takes_dyn(&Empty);
        let rev = Revision {
            hash: "sha256:aa".into(),
            parents: vec![],
            author: serde_yaml::Value::Null,
            timestamp: None,
            signed: false,
        };
        assert_eq!(rev.parents.len(), 0);
        assert!(matches!(AuthorVerdict::Unsigned, AuthorVerdict::Unsigned));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p allod-substrate`
Expected: FAIL to compile — package `allod-substrate` does not exist yet / types unresolved.

- [ ] **Step 3: Write minimal implementation**

`crates/allod-substrate/Cargo.toml`:

```toml
[package]
name = "allod-substrate"
version = "0.1.0"
edition = "2021"

[dependencies]
allod-core = { path = "../allod-core" }
serde_yaml = "0.9"
```

(Match the exact `serde_yaml` version other crates use — check `crates/allod-core/Cargo.toml` and copy it verbatim.)

Top of `crates/allod-substrate/src/lib.rs`, above the test module:

```rust
//! The abstract changeset substrate (§3.1): the four properties every
//! substrate provides — content-addressed revisions, a parent-pointer
//! DAG, signable authorship, and deterministic state. Parts 4 to 6 of
//! the spec are written against this interface; `NativeSubstrate`
//! adapts the native log (§3.2) and a git binding (§3.3) follows in
//! its own crate.

pub mod conformance;
pub mod native;

/// A revision hash, algorithm-prefixed (§1.7), e.g. `sha256:…`.
pub type RevHash = String;
/// A branch-head name, e.g. `HEAD` or `refs/heads/main`.
pub type RefName = String;

/// One revision's envelope, substrate-neutral (§3.1).
pub struct Revision {
    pub hash: RevHash,
    /// At least one entry except genesis; more than one is a merge.
    pub parents: Vec<RevHash>,
    /// Substrate-specific author record, preserved verbatim
    /// (native: principal-ref + key id; git: committer + key).
    pub author: serde_yaml::Value,
    pub timestamp: Option<String>,
    pub signed: bool,
}

/// Outcome of checking a revision's authorship signature (§3.1 property 3).
pub enum AuthorVerdict {
    Verified { principal: String, key_id: String },
    Unsigned,
    Failed(String),
}

/// The §3.1 interface. Implementations MUST enforce content
/// addressing on read: `revision(h)` fails when the stored content
/// does not recompute to `h`.
pub trait Substrate {
    fn revision(&self, hash: &str) -> Result<Revision, String>;
    /// The revision's operation set, deterministic (§3.2.2 native,
    /// §3.3 git tree diff).
    fn operation_set(&self, hash: &str) -> Result<Vec<serde_yaml::Value>, String>;
    /// State hash at this revision (§3.1 property 4).
    fn state_hash(&self, hash: &str) -> Result<String, String>;
    fn heads(&self) -> Result<Vec<(RefName, RevHash)>, String>;
    fn verify_authorship(&self, hash: &str) -> Result<AuthorVerdict, String>;
}
```

For this task only, create `src/conformance.rs` and `src/native.rs` as empty files (`//! placeholder module docs` one-liners get replaced in Tasks 2 and 3; if you prefer, add the `pub mod` lines in those tasks instead — either way the crate must compile here).

Add `"crates/allod-substrate"` to the workspace `members` list in the root `Cargo.toml`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p allod-substrate`
Expected: PASS (1 test).
Run: `cargo test --workspace`
Expected: PASS, no existing test modified.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/allod-substrate
git commit -m "feat(substrate): allod-substrate crate with the §3.1 Substrate trait"
```

---

### Task 2: Conformance suite

**Files:**
- Create: `crates/allod-substrate/src/conformance.rs` (replacing the placeholder)
- Test: same file, `#[cfg(test)]` module with an in-memory fake

**Interfaces:**
- Consumes: `Substrate`, `AuthorVerdict`, `RevHash` from Task 1.
- Produces: `pub fn check_conformance(sub: &dyn Substrate, signed_rev: &str) -> Result<(), String>` — Task 4 calls this against `NativeSubstrate`, milestone 2 against `GitSubstrate`.

Fixture preconditions (document them in the fn's doc comment): at least one head; the head revision's operation set is non-empty and creates at least one object (so its state differs from its parent's); `signed_rev` names a revision whose authorship must verify.

- [ ] **Step 1: Write the failing test**

In `crates/allod-substrate/src/conformance.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AuthorVerdict, Revision, Substrate};
    use std::collections::BTreeMap;

    /// Minimal well-behaved substrate: a two-revision chain.
    struct Fake {
        revs: BTreeMap<String, (Vec<String>, String)>, // hash -> (parents, state_hash)
        head: String,
    }

    impl Fake {
        fn good() -> Fake {
            let mut revs = BTreeMap::new();
            revs.insert("sha256:genesis".to_string(), (vec![], "sha256:s0".to_string()));
            revs.insert(
                "sha256:tip".to_string(),
                (vec!["sha256:genesis".to_string()], "sha256:s1".to_string()),
            );
            Fake { revs, head: "sha256:tip".to_string() }
        }
    }

    impl Substrate for Fake {
        fn revision(&self, hash: &str) -> Result<Revision, String> {
            let (parents, _) = self.revs.get(hash).ok_or("unknown revision")?;
            Ok(Revision {
                hash: hash.to_string(),
                parents: parents.clone(),
                author: serde_yaml::Value::Null,
                timestamp: None,
                signed: true,
            })
        }
        fn operation_set(&self, _hash: &str) -> Result<Vec<serde_yaml::Value>, String> {
            Ok(vec![serde_yaml::from_str("{ create: { kind: node, id: a } }").unwrap()])
        }
        fn state_hash(&self, hash: &str) -> Result<String, String> {
            Ok(self.revs.get(hash).ok_or("unknown revision")?.1.clone())
        }
        fn heads(&self) -> Result<Vec<(String, String)>, String> {
            Ok(vec![("HEAD".to_string(), self.head.clone())])
        }
        fn verify_authorship(&self, _hash: &str) -> Result<AuthorVerdict, String> {
            Ok(AuthorVerdict::Verified { principal: "principal:o".into(), key_id: "k".into() })
        }
    }

    #[test]
    fn good_substrate_conforms() {
        let fake = Fake::good();
        check_conformance(&fake, "sha256:tip").unwrap();
    }

    #[test]
    fn cyclic_parent_dag_fails() {
        let mut fake = Fake::good();
        // tip's parent points back at tip: a cycle.
        fake.revs.insert(
            "sha256:genesis".to_string(),
            (vec!["sha256:tip".to_string()], "sha256:s0".to_string()),
        );
        let err = check_conformance(&fake, "sha256:tip").unwrap_err();
        assert!(err.contains("cycle"), "got: {err}");
    }

    #[test]
    fn wrong_hash_identity_fails() {
        struct Lying(Fake);
        impl Substrate for Lying {
            fn revision(&self, hash: &str) -> Result<Revision, String> {
                let mut r = self.0.revision(hash)?;
                r.hash = "sha256:other".to_string(); // content-address violation
                Ok(r)
            }
            fn operation_set(&self, h: &str) -> Result<Vec<serde_yaml::Value>, String> {
                self.0.operation_set(h)
            }
            fn state_hash(&self, h: &str) -> Result<String, String> { self.0.state_hash(h) }
            fn heads(&self) -> Result<Vec<(String, String)>, String> { self.0.heads() }
            fn verify_authorship(&self, h: &str) -> Result<AuthorVerdict, String> {
                self.0.verify_authorship(h)
            }
        }
        let err = check_conformance(&Lying(Fake::good()), "sha256:tip").unwrap_err();
        assert!(err.contains("identity"), "got: {err}");
    }

    #[test]
    fn nondeterministic_state_fails() {
        struct Flaky { inner: Fake, calls: std::cell::Cell<u32> }
        impl Substrate for Flaky {
            fn revision(&self, h: &str) -> Result<Revision, String> { self.inner.revision(h) }
            fn operation_set(&self, h: &str) -> Result<Vec<serde_yaml::Value>, String> {
                self.inner.operation_set(h)
            }
            fn state_hash(&self, h: &str) -> Result<String, String> {
                let n = self.calls.get();
                self.calls.set(n + 1);
                Ok(format!("sha256:varies-{n}-{h}"))
            }
            fn heads(&self) -> Result<Vec<(String, String)>, String> { self.inner.heads() }
            fn verify_authorship(&self, h: &str) -> Result<AuthorVerdict, String> {
                self.inner.verify_authorship(h)
            }
        }
        let flaky = Flaky { inner: Fake::good(), calls: std::cell::Cell::new(0) };
        let err = check_conformance(&flaky, "sha256:tip").unwrap_err();
        assert!(err.contains("deterministic"), "got: {err}");
    }

    #[test]
    fn unverified_authorship_fails() {
        struct NoSig(Fake);
        impl Substrate for NoSig {
            fn revision(&self, h: &str) -> Result<Revision, String> { self.0.revision(h) }
            fn operation_set(&self, h: &str) -> Result<Vec<serde_yaml::Value>, String> {
                self.0.operation_set(h)
            }
            fn state_hash(&self, h: &str) -> Result<String, String> { self.0.state_hash(h) }
            fn heads(&self) -> Result<Vec<(String, String)>, String> { self.0.heads() }
            fn verify_authorship(&self, _h: &str) -> Result<AuthorVerdict, String> {
                Ok(AuthorVerdict::Failed("bad signature".into()))
            }
        }
        let err = check_conformance(&NoSig(Fake::good()), "sha256:tip").unwrap_err();
        assert!(err.contains("authorship"), "got: {err}");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p allod-substrate conformance`
Expected: FAIL to compile — `check_conformance` not defined.

- [ ] **Step 3: Write the implementation**

Above the test module in `conformance.rs`:

```rust
//! Substrate conformance (§3.1): one suite, run against every
//! implementation. Native runs it in this workspace; the git binding
//! (milestone 2) runs the same checks.

use crate::{AuthorVerdict, Substrate};
use std::collections::BTreeSet;

/// Walk limit: a conformance fixture is small; hitting this means a
/// broken parent DAG, not a big history.
const MAX_WALK: usize = 10_000;

/// Check the four §3.1 properties against a live fixture.
///
/// Preconditions on the fixture: at least one head; the head
/// revision's operation set is non-empty and creates at least one
/// object (so head state differs from parent state); `signed_rev`
/// names a revision whose authorship must verify.
pub fn check_conformance(sub: &dyn Substrate, signed_rev: &str) -> Result<(), String> {
    let heads = sub.heads()?;
    if heads.is_empty() {
        return Err("conformance: substrate reports no heads".into());
    }

    for (name, head) in &heads {
        // Property 1: content-addressed revisions.
        let rev = sub.revision(head)?;
        if rev.hash != *head {
            return Err(format!(
                "conformance: revision identity mismatch at head {name}: asked {head}, got {}",
                rev.hash
            ));
        }
        if !allod_core::has_algo_prefix(&rev.hash) {
            return Err(format!(
                "conformance: revision hash {} lacks an algorithm prefix (§1.7)",
                rev.hash
            ));
        }

        // Property 2: parent-pointer DAG — walk to genesis, no cycles.
        let mut frontier = vec![head.clone()];
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut steps = 0usize;
        while let Some(h) = frontier.pop() {
            if !seen.insert(h.clone()) {
                return Err(format!("conformance: parent DAG cycle at {h}"));
            }
            steps += 1;
            if steps > MAX_WALK {
                return Err("conformance: parent walk exceeded limit (broken DAG?)".into());
            }
            let r = sub.revision(&h)?;
            for p in r.parents {
                if !seen.contains(&p) {
                    frontier.push(p);
                }
            }
        }

        // Property 4: deterministic state.
        let s1 = sub.state_hash(head)?;
        let s2 = sub.state_hash(head)?;
        if s1 != s2 {
            return Err(format!(
                "conformance: state hash at {head} is not deterministic ({s1} vs {s2})"
            ));
        }
        let head_rev = sub.revision(head)?;
        if let Some(parent) = head_rev.parents.first() {
            if !sub.operation_set(head)?.is_empty() {
                let sp = sub.state_hash(parent)?;
                if sp == s1 {
                    return Err(format!(
                        "conformance: head {head} has operations but the same state as its parent"
                    ));
                }
            }
        }
    }

    // Property 3: signable authorship.
    match sub.verify_authorship(signed_rev)? {
        AuthorVerdict::Verified { .. } => Ok(()),
        AuthorVerdict::Unsigned => Err(format!(
            "conformance: authorship of {signed_rev} is unsigned; fixture promised a signed revision"
        )),
        AuthorVerdict::Failed(reason) => Err(format!(
            "conformance: authorship of {signed_rev} failed: {reason}"
        )),
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p allod-substrate`
Expected: PASS (6 tests). Then `cargo test --workspace`: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/allod-substrate/src/conformance.rs
git commit -m "feat(substrate): §3.1 conformance suite with self-tests"
```

---

### Task 3: `Graph::fold_to` — fold stopping at a revision

`NativeSubstrate::state_hash(rev)` needs the state *as of* `rev`, but `Graph::fold` (crates/allod-core/src/store.rs:378) always folds the full chain. Add a bounded variant; `fold()` becomes a wrapper. This is the only `allod-core` change in the milestone.

**Files:**
- Modify: `crates/allod-core/src/store.rs:378-439` (the `fold` fn)
- Test: `#[cfg(test)]` module in `store.rs` (add to the existing one if present, otherwise create)

**Interfaces:**
- Produces: `pub fn fold_to(&self, stop: Option<&str>) -> Result<State, String>` on `Graph` — fold the chain in order and stop *after* applying the changeset whose `hash` equals `stop`; `None` folds everything. Error if `stop` names a hash not in the chain. `pub fn fold(&self)` delegates to `self.fold_to(None)` and keeps its exact current behavior.

- [ ] **Step 1: Write the failing test**

Read `crates/allod-core/src/store.rs:378-439` first to see how `fold` builds its registry per changeset — `fold_to` must reuse that loop, not reimplement it. Then add a test. Build the fixture with the same helpers the existing store/ops tests use (see `crates/allod-graph/src/ops.rs` test module for the `raw_changeset` pattern; if store.rs has its own test fixtures, prefer those):

```rust
#[test]
fn fold_to_stops_at_the_named_revision() {
    // Arrange: a graph with two changesets cs1 (genesis, creates node a)
    // and cs2 (creates node b), built exactly like the existing fold tests
    // in this file / in allod-graph. Adapt the fixture helpers you find.
    // (Use Graph::with_store(Box::new(MemStore::default())) — no filesystem.)
    //
    // Assert:
    let full = graph.fold_to(None).unwrap();
    let at_cs1 = graph.fold_to(Some(&cs1_hash)).unwrap();
    assert!(full.get_live("node", "b").is_some());
    assert!(at_cs1.get_live("node", "b").is_none(), "cs2 must not be applied");
    assert!(at_cs1.get_live("node", "a").is_some());
    assert_eq!(
        graph.fold().unwrap().state_hash().unwrap(),
        full.state_hash().unwrap(),
        "fold() must equal fold_to(None)"
    );
    let err = graph.fold_to(Some("sha256:nope")).unwrap_err();
    assert!(err.contains("not in the chain"), "got: {err}");
}
```

The comment block above is fixture *guidance*, not literal code — write the actual arrange section against the helpers that exist in the file. The four assertions are the contract and go in verbatim.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p allod-core fold_to`
Expected: FAIL to compile — `fold_to` not defined.

- [ ] **Step 3: Implement**

Rename the body of `fold` to `fold_to(&self, stop: Option<&str>)`; inside its per-changeset loop, after applying a changeset whose `hash` matches `stop`, break; after the loop, if `stop` was `Some` and never matched, return `Err(format!("revision {stop} is not in the chain"))`. Then:

```rust
pub fn fold(&self) -> Result<State, String> {
    self.fold_to(None)
}
```

Keep every line of the existing loop otherwise untouched.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p allod-core` then `cargo test --workspace`
Expected: PASS, including every pre-existing fold test.

- [ ] **Step 5: Commit**

```bash
git add crates/allod-core/src/store.rs
git commit -m "feat(core): Graph::fold_to folds the chain up to a named revision"
```

---

### Task 4: `NativeSubstrate` adapter plus conformance against a real graph

**Files:**
- Create: `crates/allod-substrate/src/native.rs` (replacing the placeholder)
- Modify: `crates/allod-substrate/Cargo.toml` (add dev-dependency `allod-graph = { path = "../allod-graph" }`)
- Test: `crates/allod-substrate/tests/native_conformance.rs`

**Interfaces:**
- Consumes: `allod_core::store::Graph` (`read_changeset`, `head`, `fold_to`), `allod_core::model::changeset_hash`, `allod_core::sign::verify`, `State::{find_principal, public_key_of}`; `check_conformance` from Task 2.
- Produces: `pub struct NativeSubstrate<'g> { … }` with `pub fn new(graph: &'g Graph) -> NativeSubstrate<'g>`, implementing `Substrate`. Task 5 and milestone 2 consume this.

- [ ] **Step 1: Write the failing integration test**

`crates/allod-substrate/tests/native_conformance.rs`:

```rust
//! The native log passes the same §3.1 conformance suite the git
//! binding will run in milestone 2.

use allod_substrate::conformance::check_conformance;
use allod_substrate::native::NativeSubstrate;
use allod_substrate::Substrate;

#[test]
fn native_substrate_conforms() {
    // Build a real graph in memory the same way the flows tests do:
    // follow the init/setup pattern at the top of
    // crates/allod-graph/tests/flows.rs (init an owner, then commit at
    // least one signed changeset that creates a node). Capture the head
    // hash as `tip`.
    let (graph, tip) = fixture_graph();

    let sub = NativeSubstrate::new(&graph);
    check_conformance(&sub, &tip).expect("native substrate must conform to §3.1");

    // Adapter specifics beyond the generic suite:
    let rev = sub.revision(&tip).unwrap();
    assert!(!rev.parents.is_empty(), "tip is not genesis");
    assert!(rev.signed);
    let ops = sub.operation_set(&tip).unwrap();
    assert!(!ops.is_empty());
}

#[test]
fn revision_read_enforces_content_addressing() {
    let (graph, tip) = fixture_graph();
    let sub = NativeSubstrate::new(&graph);
    // Asking for a hash that isn't in the log fails cleanly.
    assert!(sub.revision("sha256:0000").is_err());
    // And the stored tip recomputes to its own hash (revision() would
    // error otherwise — see trait doc).
    assert_eq!(sub.revision(&tip).unwrap().hash, tip);
}
```

`fixture_graph()` is a helper in this test file returning `(Graph, String)`; write it by mirroring the setup in `crates/allod-graph/tests/flows.rs` (use `allod_graph::flows` init + a commit; in-memory `MemStore`, no filesystem).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p allod-substrate --test native_conformance`
Expected: FAIL to compile — `native::NativeSubstrate` not defined.

- [ ] **Step 3: Implement the adapter**

`crates/allod-substrate/src/native.rs`:

```rust
//! `NativeSubstrate` (§3.2): the native changeset log presented
//! through the §3.1 interface. Pure adapter — no new semantics; every
//! answer comes from `allod_core::store::Graph`.

use crate::{AuthorVerdict, Revision, Substrate};
use allod_core::get_str;
use allod_core::model::changeset_hash;
use allod_core::sign;
use allod_core::store::Graph;
use serde_yaml::Value;

pub struct NativeSubstrate<'g> {
    graph: &'g Graph,
}

impl<'g> NativeSubstrate<'g> {
    pub fn new(graph: &'g Graph) -> NativeSubstrate<'g> {
        NativeSubstrate { graph }
    }

    /// Read a changeset and enforce content addressing (§3.1 property 1):
    /// the stored bytes must recompute to the requested hash.
    fn read_checked(&self, hash: &str) -> Result<Value, String> {
        let cs = self.graph.read_changeset(hash)?;
        let (computed, _, _, _) = changeset_hash(&cs)?;
        if computed != hash {
            return Err(format!(
                "revision identity mismatch: {hash} stored, {computed} recomputed"
            ));
        }
        Ok(cs)
    }
}

impl Substrate for NativeSubstrate<'_> {
    fn revision(&self, hash: &str) -> Result<Revision, String> {
        let cs = self.read_checked(hash)?;
        let parents = cs
            .get("parents")
            .and_then(Value::as_sequence)
            .map(|s| s.iter().filter_map(Value::as_str).map(String::from).collect())
            .unwrap_or_default();
        Ok(Revision {
            hash: hash.to_string(),
            parents,
            author: cs.get("author").cloned().unwrap_or(Value::Null),
            timestamp: get_str(&cs, "timestamp").map(String::from),
            signed: cs.get("signature").is_some(),
        })
    }

    fn operation_set(&self, hash: &str) -> Result<Vec<Value>, String> {
        let cs = self.read_checked(hash)?;
        Ok(cs
            .get("operations")
            .and_then(Value::as_sequence)
            .cloned()
            .unwrap_or_default())
    }

    fn state_hash(&self, hash: &str) -> Result<String, String> {
        self.graph.fold_to(Some(hash))?.state_hash()
    }

    fn heads(&self) -> Result<Vec<(String, String)>, String> {
        Ok(self.graph.head()?.map(|h| vec![("HEAD".to_string(), h)]).unwrap_or_default())
    }

    fn verify_authorship(&self, hash: &str) -> Result<AuthorVerdict, String> {
        let cs = self.read_checked(hash)?;
        let Some(signature) = get_str(&cs, "signature").map(String::from) else {
            return Ok(AuthorVerdict::Unsigned);
        };
        let author = cs.get("author").cloned().unwrap_or(Value::Null);
        let principal = get_str(&author, "principal").unwrap_or("").to_string();
        let key_id = get_str(&author, "key").unwrap_or("").to_string();

        // Key lookup state: the revision's first parent, matching
        // flows::verify. Genesis self-registers its author, so fall
        // back to the state at the revision itself.
        let rev = self.revision(hash)?;
        let state = match rev.parents.first() {
            Some(parent) => self.graph.fold_to(Some(parent))?,
            None => self.graph.fold_to(Some(hash))?,
        };
        let state = if state.public_key_of(&principal, &key_id).is_some() {
            state
        } else {
            self.graph.fold_to(Some(hash))?
        };
        let Some(public) = state.public_key_of(&principal, &key_id) else {
            return Ok(AuthorVerdict::Failed(format!(
                "no active key {key_id} for {principal}"
            )));
        };
        match sign::verify(&public, hash, &signature) {
            Ok(()) => Ok(AuthorVerdict::Verified { principal, key_id }),
            Err(e) => Ok(AuthorVerdict::Failed(e)),
        }
    }
}
```

Before finalizing, read how `flows::verify` (crates/allod-graph/src/flows.rs:830 onward) verifies a changeset signature — the message it passes to `sign::verify` and where it fetches the key — and match it exactly (if it signs over the bare hash string, the code above is right; if it uses a different preimage, mirror that). The adapter must agree with `flows::verify` on every fixture.

Add to `crates/allod-substrate/Cargo.toml`:

```toml
[dev-dependencies]
allod-graph = { path = "../allod-graph" }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p allod-substrate` then `cargo test --workspace`
Expected: PASS. The native log now passes the same suite the git binding will face.

- [ ] **Step 5: Commit**

```bash
git add crates/allod-substrate
git commit -m "feat(substrate): NativeSubstrate adapter; native log passes §3.1 conformance"
```

---

### Task 5: `flows::verify` integrity and authorship through the trait

Rewire one real consumer so the abstraction carries production traffic. `flows::verify` currently recomputes hashes and checks signatures inline per changeset; its integrity and authorship levels move to `NativeSubstrate`. Governance (policy replay) stays exactly as it is. The verify report must be identical before and after.

**Files:**
- Modify: `crates/allod-graph/Cargo.toml` (add dependency `allod-substrate = { path = "../allod-substrate" }`)
- Modify: `crates/allod-graph/src/flows.rs:830+` (the `verify` fn)
- Test: `crates/allod-graph/tests/flows.rs` (one new test; existing tests untouched)

**Interfaces:**
- Consumes: `NativeSubstrate::new(&graph)`, `Substrate::{revision, verify_authorship}`, `AuthorVerdict` from Tasks 1 and 4.
- Produces: no new public API — `pub fn verify(graph: &Graph) -> Result<VerifyReport, AllodError>` keeps its exact signature and output.

- [ ] **Step 1: Capture the before-behavior with a pinning test**

In `crates/allod-graph/tests/flows.rs`, add (using the file's existing fixture helpers):

```rust
#[test]
fn verify_report_is_stable_across_substrate_rewire() {
    // Fixture: init + one admitted commit + one tampered copy.
    // 1. A clean graph verifies ok, and every changeset entry reports
    //    integrity and authorship Verified.
    // 2. Corrupt one stored changeset file's operations (via the store),
    //    and verify reports an integrity failure for that hash.
    // Write these two assertions against the current implementation and
    // run them BEFORE touching flows::verify — they pass now and must
    // still pass after the rewire.
}
```

Fill the body with the file's real helpers (this is the pinning test — its content asserts today's behavior; read neighboring tests for how to reach the store and corrupt a changeset, e.g. re-writing `.allod/changesets/<hash>.yaml` through the `DocStore`).

- [ ] **Step 2: Run to verify it passes against the CURRENT code**

Run: `cargo test -p allod-graph verify_report_is_stable`
Expected: PASS (this is a pinning test, inverted TDD: green before the change, must stay green after).

- [ ] **Step 3: Rewire**

In `flows::verify`, construct `let sub = NativeSubstrate::new(graph);` before the loop. For each changeset in the chain:

- Integrity level: replace the inline hash recomputation with `sub.revision(&hash)` — `Ok(_)` means integrity verified (content addressing is enforced on read); `Err(reason)` becomes `LevelResult::Failed(reason)`.
- Authorship level: replace the inline key lookup + `sign::verify` call with `sub.verify_authorship(&hash)`, mapping `Verified { .. }` → `LevelResult::Verified`, `Unsigned` → whatever the current code reports for a missing signature (match it exactly), `Failed(reason)` → `LevelResult::Failed(reason)`.
- Leave the governance level, evidence handling, state advancement, and report assembly untouched.

Delete only the lines the two calls replace. If the current code's failure strings differ from the adapter's, keep the report's `ok`/`Failed` *structure* identical and update string assertions only in the pinning test if a message text legitimately changed — the levels' verdicts must not change on any fixture.

- [ ] **Step 4: Run the full suite**

Run: `cargo test --workspace`
Expected: PASS — the pinning test, every pre-existing flows/ops/wasm test, everything.

- [ ] **Step 5: Commit**

```bash
git add crates/allod-graph
git commit -m "refactor(graph): verify integrity+authorship through the Substrate trait"
```

---

### Task 6: Milestone close-out

**Files:**
- Modify: `docs/superpowers/specs/2026-08-04-governed-code-review-design.md` (mark milestone 1 done in the Milestones section)

- [ ] **Step 1: Full workspace check**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings 2>/dev/null || cargo test --workspace`
Expected: tests PASS (clippy advisory — fix new-code lints, leave pre-existing ones).

- [ ] **Step 2: Confirm wasm still builds**

Run: `cargo check -p allod-wasm --target wasm32-unknown-unknown` (if the target is installed; otherwise `cargo check -p allod-wasm`)
Expected: clean — `allod-substrate` introduced no wasm-hostile deps into the graph crate.

- [ ] **Step 3: Update the design doc milestone status and commit**

```bash
git add docs/superpowers/specs/2026-08-04-governed-code-review-design.md
git commit -m "docs: milestone 1 (substrate refactor) complete"
```
