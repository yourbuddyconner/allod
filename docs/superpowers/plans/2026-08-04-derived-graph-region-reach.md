# Derived Graph + Region Reach (Milestone 3) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Region rules reach git commits through the derived code graph — extend the existing `repo.rs` derivation to all languages with deletion handling, expose it as `allod git index`, implement §8.3 file-granular region reach in `evaluate_git`, and dogfood it on the allod repo itself.

**Architecture:** The derivation stays in `crates/allod-graph/src/repo.rs` (extended in place — extracting a crate adds nothing yet). Region reach lives in `allod-core::policy::evaluate_git` behind an optional derived-state context, so callers without a graph keep today's behavior. The derived graph is **materialized and committed by the owner** (the indexer signs; CI has no keys) and CI evaluates against the committed state — a recorded deviation from the design doc's "CI recomputes on demand" line. The indexer skips `.allod/` (the graph must not index itself — every materialization commit would otherwise trigger endless re-index churn).

**Tech Stack:** Rust 2021 workspace, serde_yaml, git CLI. No new dependencies.

## Global Constraints

- Zero behavior change to native policy paths and existing CLI commands; every pre-existing workspace test passes unmodified (except signatures explicitly changed here: `evaluate_git` gains a third parameter — update its existing callers and tests mechanically).
- Error idiom `Result<T, String>` in allod-core; `AllodError` at allod-graph/CLI layers.
- §8.3 reach rule verbatim: a region rule reaches a substrate changeset when the commit's deterministic operation set touches a path from which an in-region derived object derives, evaluated against the derived graph state; matching MUST NOT depend on symbol or span identity across commits.
- §8.4: an unchanged entity must not update just because the deriving commit moved (the existing `content_differs` rule stays authoritative).
- Derived objects carry lineage: `derived_from` (`git:#<commit>`), `derived_by`, `method: deterministic`, `tool: allod-scan@0.1` — the existing `provenance()` shape.
- The indexer never emits ops for paths under `.allod/`.
- Never commit private keys; `.allod/keys/` stays gitignored and untouched.
- Worktree: all work on branch `worktree-governed-review-m3`.

---

### Task 1: Milestone-2 riders — `--graph` default reconciliation + Unsigned conformance self-test

**Files:**
- Modify: `crates/allod/src/gitcmd.rs` (the `--graph` default; find `repo_dir.join(".allod")`)
- Modify: `.github/workflows/governance.yml` (drop the now-redundant `--graph .`)
- Modify: `governance/README.md` (if it documents the `--graph .` requirement, simplify)
- Modify: `crates/allod-substrate/src/conformance.rs` (test module)
- Test: existing `crates/allod/tests/gitcmd.rs` (must pass unchanged), new conformance test

**Interfaces:**
- Produces: `git eval`/`git decide`/`git index` (Task 3) default `--graph` to the REPO DIR itself; `Graph::open` appends `.allod` internally (crates/allod-core/src/docstore.rs behavior), so `allod git eval . HEAD …` works with no `--graph` flag. An explicit `--graph <dir>` keeps meaning "the directory containing `.allod/`".

- [ ] **Step 1: Reproduce the footgun as a failing expectation.** In `crates/allod/tests/gitcmd.rs`, find the e2e test's eval invocations and REMOVE the `--graph` arguments (they currently pass the graph parent dir explicitly if present; if the test already relies on defaults, add one invocation without `--graph` that exercises the default). Run `cargo test -p allod --test gitcmd` — expected: FAIL ("not an allod graph"), demonstrating the double-append default.

- [ ] **Step 2: Fix the default.** In `gitcmd.rs`, change the `--graph` default from `repo_dir.join(".allod")` to `repo_dir.to_path_buf()`, and update the flag's usage/help text to say the value is the directory that CONTAINS `.allod/`. Run the test — expected: PASS.

- [ ] **Step 3: Simplify callers.** In `.github/workflows/governance.yml`, remove `--graph .` from the eval invocation (keep `--repo allod`). In `governance/README.md`, update any `--graph .` instructions to the flagless form.

- [ ] **Step 4: Unsigned conformance self-test.** In `crates/allod-substrate/src/conformance.rs`'s test module, add:

```rust
#[test]
fn unsigned_authorship_fails_conformance() {
    struct Unsigned(Fake);
    impl Substrate for Unsigned {
        fn revision(&self, h: &str) -> Result<Revision, String> { self.0.revision(h) }
        fn operation_set(&self, h: &str) -> Result<Vec<serde_yaml::Value>, String> {
            self.0.operation_set(h)
        }
        fn state_hash(&self, h: &str) -> Result<String, String> { self.0.state_hash(h) }
        fn heads(&self) -> Result<Vec<(String, String)>, String> { self.0.heads() }
        fn verify_authorship(&self, _h: &str) -> Result<AuthorVerdict, String> {
            Ok(AuthorVerdict::Unsigned)
        }
    }
    let err = check_conformance(&Unsigned(Fake::good()), "sha256:tip").unwrap_err();
    assert!(err.contains("unsigned"), "got: {err}");
}
```

Run: `cargo test -p allod-substrate` — expected: PASS (the suite already treats `Unsigned` for the promised signed rev as failure; this pins it).

- [ ] **Step 5: Full suite + commit**

Run: `cargo test --workspace` — expected: PASS.

```bash
git add crates/allod/src/gitcmd.rs crates/allod/tests/gitcmd.rs .github/workflows/governance.yml governance/README.md crates/allod-substrate/src/conformance.rs
git commit -m "fix(cli): --graph names the dir containing .allod; conformance pins Unsigned failure"
```

---

### Task 2: Indexer extension — all-language files, deletions, `.allod/` exclusion

**Files:**
- Modify: `crates/allod-graph/src/repo.rs` (`ExistingIndex`, `index_state`, `import_commit`)
- Test: `crates/allod-graph/tests/repo_index.rs` (new integration test file)

**Interfaces:**
- Consumes: existing `import_commit(graph, repo, commit, indexer) -> Result<(String, bool), AllodError>` (signature unchanged), `make_sample_repo` pattern for fixtures, `flows::init` for graph setup (mirror `crates/allod-graph/tests/flows.rs`).
- Produces (Tasks 3 and 5 rely on): `import_commit` now (a) emits a `code/SourceFile` node for EVERY blob at the commit except paths starting with `.allod/`, with `language` from the extension map below (attribute omitted for unknown extensions); (b) keeps Rust item extraction for `.rs` files exactly as today; (c) deletes derived objects whose source vanished: files gone from the tree → delete the SourceFile node plus its `in_repo` edge plus its declared items (their nodes, `declares` edges, and any `calls` edges touching them); items gone from a still-present file → delete the item node, its `declares` edge, and its `calls` edges.

Language map (a private `fn language_of(path: &str) -> Option<&'static str>`): `rs`→rust, `ts`/`tsx`→typescript, `js`/`jsx`→javascript, `py`→python, `toml`→toml, `yaml`/`yml`→yaml, `md`→markdown, `sh`→shell, `json`→json; anything else → `None`.

Implementation notes:
- `ExistingIndex` must additionally record, per file path, the `in_repo` edge (id, rev) and per item the `declares` edge (id, rev), so deletions can remove them (the fold rejects dangling edges, so edges must be deleted in the same changeset as their endpoints). Extend `index_state` to collect these.
- The `ls-tree` loop drops the `path.ends_with(".rs")` filter; add `if path.starts_with(".allod/") { continue; }`. Only fetch source text (`git show`) for `.rs` paths — other files never need content, only path+blob.
- Deletion ops use the existing `delete` payload shape: `{ delete: { kind: node|edge, id, prior: <rev> } }` (see `import_commit`'s existing call-edge deletion for the exact shape).
- `import_commit` returning `Err("nothing changed at this commit")` stays as-is; the CLI (Task 3) maps it to a friendly message.

- [ ] **Step 1: Write the failing tests** (`crates/allod-graph/tests/repo_index.rs`):

```rust
//! Indexer extension (§8.3): all-language file granularity, deletion
//! handling, .allod/ exclusion.

use allod_core::get_str;
use allod_graph::repo::import_commit;
use std::path::Path;
use std::process::Command;

// Fixture helpers: mirror the init pattern from tests/flows.rs to build
// an in-memory-persisted graph (or tempdir graph) with an owner
// principal named "owner"; and a git fixture repo builder like
// tests in allod-substrate-git (git init -b main, user config,
// commit.gpgsign false). Write helpers `fixture_graph()` and
// `git_fixture()` accordingly at the top of this file.

fn sh(dir: &Path, args: &[&str]) {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn indexes_every_language_and_skips_dot_allod() {
    let (graph, _owner) = fixture_graph();
    let repo = git_fixture(); // tempdir with git init done
    std::fs::create_dir_all(repo.path().join("src")).unwrap();
    std::fs::create_dir_all(repo.path().join(".allod")).unwrap();
    std::fs::write(repo.path().join("src/lib.rs"), "pub fn a() {}\n").unwrap();
    std::fs::write(repo.path().join("web.ts"), "export const x = 1;\n").unwrap();
    std::fs::write(repo.path().join("README.md"), "# hi\n").unwrap();
    std::fs::write(repo.path().join("data.bin"), [0u8, 1, 2]).unwrap();
    std::fs::write(repo.path().join(".allod/graph.yaml"), "graph_id: x\n").unwrap();
    sh(repo.path(), &["add", "-A"]);
    sh(repo.path(), &["commit", "-q", "-m", "c1"]);

    import_commit(&graph, repo.path(), "HEAD", "owner").unwrap();
    let state = graph.fold().unwrap();

    let paths: Vec<(String, Option<String>)> = state
        .objects
        .iter()
        .filter(|((k, _), o)| {
            k == "node"
                && !o.deleted
                && get_str(&o.content, "type").map(allod_core::bare) == Some("code/SourceFile")
        })
        .map(|(_, o)| {
            let attrs = o.content.get("attributes").unwrap();
            (
                get_str(attrs, "path").unwrap().to_string(),
                get_str(attrs, "language").map(String::from),
            )
        })
        .collect();

    let find = |p: &str| paths.iter().find(|(path, _)| path == p);
    assert_eq!(find("src/lib.rs").unwrap().1.as_deref(), Some("rust"));
    assert_eq!(find("web.ts").unwrap().1.as_deref(), Some("typescript"));
    assert_eq!(find("README.md").unwrap().1.as_deref(), Some("markdown"));
    assert_eq!(find("data.bin").unwrap().1, None, "unknown extension: no language attr");
    assert!(find(".allod/graph.yaml").is_none(), ".allod/ is never indexed");
    // Rust item extraction still works.
    let has_fn_a = state.objects.iter().any(|((k, _), o)| {
        k == "node"
            && !o.deleted
            && get_str(&o.content, "type").map(allod_core::bare) == Some("code/Function")
            && o.content.get("attributes").and_then(|a| get_str(a, "name")) == Some("a")
    });
    assert!(has_fn_a);
}

#[test]
fn deletions_propagate_files_items_and_edges() {
    let (graph, _owner) = fixture_graph();
    let repo = git_fixture();
    std::fs::create_dir_all(repo.path().join("src")).unwrap();
    std::fs::write(repo.path().join("src/a.rs"), "pub fn f() {}\npub fn g() { f() }\n").unwrap();
    std::fs::write(repo.path().join("doomed.md"), "bye\n").unwrap();
    sh(repo.path(), &["add", "-A"]);
    sh(repo.path(), &["commit", "-q", "-m", "c1"]);
    import_commit(&graph, repo.path(), "HEAD", "owner").unwrap();

    // c2: delete doomed.md entirely; remove g() from a.rs.
    std::fs::remove_file(repo.path().join("doomed.md")).unwrap();
    std::fs::write(repo.path().join("src/a.rs"), "pub fn f() {}\n").unwrap();
    sh(repo.path(), &["add", "-A"]);
    sh(repo.path(), &["commit", "-q", "-m", "c2"]);
    import_commit(&graph, repo.path(), "HEAD", "owner").unwrap();

    let state = graph.fold().unwrap();
    let live_file = |p: &str| {
        state.objects.iter().any(|((k, _), o)| {
            k == "node"
                && !o.deleted
                && get_str(&o.content, "type").map(allod_core::bare) == Some("code/SourceFile")
                && o.content.get("attributes").and_then(|a| get_str(a, "path")) == Some(p)
        })
    };
    let live_fn = |n: &str| {
        state.objects.iter().any(|((k, _), o)| {
            k == "node"
                && !o.deleted
                && get_str(&o.content, "type").map(allod_core::bare) == Some("code/Function")
                && o.content.get("attributes").and_then(|a| get_str(a, "name")) == Some(n)
        })
    };
    assert!(!live_file("doomed.md"), "deleted file's node is tombstoned");
    assert!(live_file("src/a.rs"));
    assert!(live_fn("f"));
    assert!(!live_fn("g"), "removed item's node is tombstoned");
    // No dangling edges survive: every live edge resolves both ends.
    for ((k, id), o) in &state.objects {
        if k == "edge" && !o.deleted {
            for side in ["from", "to"] {
                let r = get_str(&o.content, side).unwrap();
                assert!(state.resolve_ref(r).is_some(), "edge {id} dangling {side}");
            }
        }
    }
    // Idempotence: re-importing the same commit yields no new ops.
    let err = import_commit(&graph, repo.path(), "HEAD", "owner").unwrap_err();
    assert!(err.to_string().contains("nothing changed"));
}
```

Write `fixture_graph()`/`git_fixture()` against the real helper patterns in `crates/allod-graph/tests/flows.rs` and `crates/allod-substrate-git/tests/git_substrate.rs` (tempfile dev-dep exists in allod-graph? add if absent).

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p allod-graph --test repo_index`
Expected: FAIL — `.ts`/`.md` files not indexed (first test), deletions not emitted (second test).

- [ ] **Step 3: Implement** per the Interfaces notes: `language_of`, ls-tree filter change + `.allod/` skip, `ExistingIndex` gaining `in_repo` and `declares` edge maps, deletion emission after the create/update passes. Keep `content_differs` semantics untouched.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p allod-graph` then `cargo test --workspace`
Expected: PASS including the pre-existing repo/demo tests (the demo flow indexes `.rs`-only fixtures, which still work — all-language just adds nodes).

- [ ] **Step 5: Commit**

```bash
git add crates/allod-graph
git commit -m "feat(index): all-language file granularity, deletion propagation, .allod exclusion"
```

---

### Task 3: `allod git index` CLI

**Files:**
- Modify: `crates/allod/src/gitcmd.rs` (new subcommand arm + fn)
- Modify: `crates/allod/src/main.rs` (usage text only, if the `git` usage string lists subcommands)
- Test: extend `crates/allod/tests/gitcmd.rs`

**Interfaces:**
- Consumes: `allod_graph::repo::import_commit`, `Graph::open`, Task 1's `--graph` default (repo dir).
- Produces: `allod git index <repo-dir> <commit-ish> --as <principal> [--graph <dir>]` — resolves the graph like eval/decide, runs `import_commit`, prints the admission outcome via the existing `print_admission` helper (or equivalent), maps the "nothing changed at this commit" error to a friendly `up to date — no derivation ops for <sha>` message with exit 0.

- [ ] **Step 1: Write the failing test** (append to `crates/allod/tests/gitcmd.rs`, reusing its `allod()`/`sh()` helpers and graph-init pattern):

```rust
#[test]
fn git_index_materializes_and_is_idempotent() {
    // Arrange: same fixture pattern as eval_decide_eval_loop_goes_green —
    // git repo + governance graph init'd with owner "conner".
    // (Reuse/extract the setup into a helper if the file doesn't have one.)
    // Then:
    // 1. allod git index <repo> HEAD --as conner        → success, admission printed
    // 2. state now has a code/SourceFile node (assert via a second index run)
    // 3. allod git index <repo> HEAD --as conner again  → success, output contains "up to date"
}
```

Fill the body with the file's real helpers; the three numbered assertions are the contract (assert on process success + stdout substrings: first run prints an admitted/held marker, second prints `up to date`).

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p allod --test gitcmd git_index`
Expected: FAIL — unknown `git index` subcommand.

- [ ] **Step 3: Implement** the subcommand following `cmd_git_eval`'s conventions (arg parsing, graph resolution, error style).

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p allod` then `cargo test --workspace` — expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/allod
git commit -m "feat(cli): allod git index — materialize the derived graph for a commit"
```

---

### Task 4: Region reach in `evaluate_git`

**Files:**
- Modify: `crates/allod-core/src/policy.rs` (`evaluate_git` + new helper; existing git tests updated mechanically for the new parameter)
- Modify: `crates/allod/src/gitcmd.rs` (eval passes the derived context)
- Test: policy.rs test module

**Interfaces:**
- Consumes: `State` (fold), `Registry` (`reg.term_closure(term)` for ancestor closure — same call `op_contexts` uses), existing `GitChange`, `glob_match`.
- Produces:
  - `evaluate_git(policy: &Value, change: &GitChange, derived: Option<(&State, &Registry)>) -> Result<Checklist, String>` — the third parameter is the derived-graph context; `None` preserves today's behavior exactly (region-keyed git rules never match).
  - Private helper `fn path_regions(state: &State, reg: &Registry) -> BTreeMap<String, BTreeSet<String>>` — for every live `code/SourceFile` node: start with the file node's own classifications; add classifications of every live node reachable from it via one live `code/declares` edge (the items it declares); expand every term through `reg.term_closure`; key by the file's `path` attribute. This is §8.3's file-granular reach: an in-region item makes its whole declaring file in-region; matching never uses symbol identity.
  - Rule semantics: a git rule with a `region:` key matches an op when `path_regions` for that op's path contains `bare(region)`. Composes with `path`/`operation` co-matching (all present op-level keys must hold on the same op).

Update the existing callers: `cmd_git_eval` builds `let state = graph.fold()?; let reg = graph.registry()?;` and passes `Some((&state, &reg))`; existing policy tests add `, None` (their behavior is unchanged by construction).

- [ ] **Step 1: Write the failing tests** (policy.rs test module — build the state fixture with the module's existing node/classification construction patterns; the `meta_registry()`/fixture registry used by other tests provides `term_closure` — if taxonomy terms need registering, mirror how existing region tests (native `evaluate` region selectors) set up their registry, and reuse their fixture if one exists):

```rust
#[test]
fn evaluate_git_region_reaches_through_derived_paths() {
    // State: SourceFile node "f1" (path "src/pay.rs"), Function node "fn1",
    // declares edge f1 -> fn1, classification on node:fn1 with term
    // "security/critical" (or the fixture taxonomy's equivalent region
    // term — reuse the exact term the native region tests use, and write
    // the policy rule against that term).
    // Policy: one rule { substrate: git, region: "<that term>" } requiring
    // reviewers role security quorum 1.
    let change_hits = GitChange {
        repo: "r".into(),
        target_ref: "refs/heads/main".into(),
        ops: vec![("update".into(), "src/pay.rs".into())],
    };
    let cl = evaluate_git(&policy, &change_hits, Some((&state, &reg))).unwrap();
    assert!(cl.matched_rules.contains("region-rule"));

    let change_misses = GitChange {
        repo: "r".into(),
        target_ref: "refs/heads/main".into(),
        ops: vec![("update".into(), "src/other.rs".into())],
    };
    let cl = evaluate_git(&policy, &change_misses, Some((&state, &reg))).unwrap();
    assert!(!cl.matched_rules.contains("region-rule"));

    // Without derived context the region rule never matches.
    let cl = evaluate_git(&policy, &change_hits, None).unwrap();
    assert!(!cl.matched_rules.contains("region-rule"));
}

#[test]
fn evaluate_git_region_reaches_file_level_classification_too() {
    // Same fixture but the classification sits on the FILE node (node:f1)
    // instead of the item. Touching src/pay.rs must still match.
}
```

(The comment blocks are fixture guidance — write the arrange sections against the module's real helpers; the assertions are the contract.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p allod-core evaluate_git`
Expected: FAIL to compile (parameter count) — then after mechanical `, None` updates to old tests, the new tests FAIL on missing region logic.

- [ ] **Step 3: Implement** `path_regions` + the `region:` arm in the rule loop + the signature change + caller updates (gitcmd eval passes `Some`).

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p allod-core` then `cargo test --workspace` — expected: PASS (the e2e gitcmd test exercises eval with the new context against a graph with no derived nodes — no behavior change there).

- [ ] **Step 5: Commit**

```bash
git add crates/allod-core crates/allod
git commit -m "feat(policy): §8.3 region reach — git rules match through derived-path classifications"
```

---

### Task 5: Dogfood — security rule, materialized graph, classification

**Files:**
- Modify: `governance/policy.yaml` (add the region rule)
- Modify: `governance/README.md` (document indexing + classification workflow)
- Modify (generated): `.allod/` (policy re-install changeset, derivation changeset, classification changeset)

**Interfaces:**
- Consumes: `allod install-policy`, `allod git index` (Task 3), `allod classify` (existing: `allod classify <dir> <node-id> <term> --as <principal>` — check its exact usage in main.rs), region reach (Task 4).

- [ ] **Step 1: Add the rule** to `governance/policy.yaml`:

```yaml
  # Region reach over the derived code graph (§8.3): any change that
  # touches a path from which a security/critical-classified subject
  # derives needs review, on every branch.
  - name: security-critical-code
    select: { substrate: git, region: "security/critical" }
    require:
      reviewers: { role: code-owner, quorum: 1 }
```

- [ ] **Step 2: Re-install the policy** (owner-signed, local): `cargo run -q -p allod -- install-policy . governance/policy.yaml --as conner` — expected: admitted (conner holds root; the set-policy op falls to root authority under the restricted posture).

- [ ] **Step 3: Materialize the derived graph**: `cargo run -q -p allod -- git index . HEAD --as conner` — expected: one large admitted derivation changeset (hundreds of ops; the whole workspace's files plus Rust items). Run it AGAIN — expected: `up to date`.

- [ ] **Step 4: Classify a security-critical subject.** Locate the `code/SourceFile` node for `crates/allod-core/src/sign.rs` (procedure: `grep -l "crates/allod-core/src/sign.rs" .allod/changesets/*.yaml`, then extract the node id from the create op — a small `python3 -c` or manual read; if the changeset format makes this awkward, report what you found). Then: `cargo run -q -p allod -- classify . <node-id> security/critical@1 --as conner --basis manual` (check `cmd_classify`'s exact flag spelling in main.rs first) — expected: admitted or held-then-approvable by conner; if held, approve with the existing `approve` command as conner.

- [ ] **Step 5: Prove reach end to end.** `cargo run -q -p allod -- git eval . HEAD --target refs/heads/main --repo allod` — HEAD of this branch touches `crates/allod-core/src/policy.rs` etc.; whether `security-critical-code` matches depends on whether the branch touches `sign.rs` — assert instead with a synthetic check: create a scratch commit touching `crates/allod-core/src/sign.rs` (add a trailing comment line), run eval on it — expected: `matched rules:` includes `security-critical-code`; then `git reset --hard HEAD~1` to drop the scratch commit. Record the observed output in your report.

- [ ] **Step 6: Update `governance/README.md`** — a short "Derived graph" section: how to materialize (`allod git index . HEAD --as conner`), that `.allod/` is excluded from indexing, how classification gives region rules reach, and that CI evaluates against the committed graph (the owner materializes; CI has no signing key). Register: plain declarative prose, mechanism only.

- [ ] **Step 7: Commit** (with the keys guard):

```bash
git add governance .allod
git status --porcelain | grep -q "\.allod/keys" && { echo "KEYS STAGED — ABORT"; exit 1; } || true
git commit -m "feat(governance): security-critical region rule; materialized derived graph + sign.rs classification"
```

Confirm via `git show --stat HEAD | grep -c keys` → 0.

- [ ] **Step 8: Workspace check**

Run: `cargo test --workspace` — expected: PASS.

---

### Task 6: Close-out — design doc + CI note

**Files:**
- Modify: `docs/superpowers/specs/2026-08-04-governed-code-review-design.md`

- [ ] **Step 1: Amend the design doc** — three edits:
1. In "### CLI and CI action", replace the paragraph beginning "In CI, derived changesets are recomputed on demand and not pushed" with: derived changesets are materialized and committed by the owner (`allod git index`), because admission requires the indexer's signing key and CI holds no keys; CI evaluates region reach against the committed graph state. Staleness between materializations is an accepted advisory-mode approximation.
2. In "### Indexer", note deletion propagation and the `.allod/` self-exclusion as shipped behavior.
3. In "## Milestones", append to milestone 3's entry: " **Status: done.** Plan: `docs/superpowers/plans/2026-08-04-derived-graph-region-reach.md`. Deviations: derivation stays in repo.rs (no allod-index-code crate yet); owner-materialized graph instead of CI recompute."

- [ ] **Step 2: Final checks**

Run: `cargo test --workspace && cargo run -q -p allod-lint -- ontologies && cargo run -q -p allod -- verify .`
Expected: all clean (verify passes on the enlarged graph).

- [ ] **Step 3: Commit**

```bash
git add docs/superpowers/specs/2026-08-04-governed-code-review-design.md
git commit -m "docs: milestone 3 (derived graph + region reach) complete"
```
