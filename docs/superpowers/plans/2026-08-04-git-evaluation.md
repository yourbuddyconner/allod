# Git Evaluation + CI Action (Milestone 2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Evaluate git commits against governance policy — `GitSubstrate`, git-selector policy evaluation, `allod git eval`/`allod git decide` CLI, a genesis governance graph for the allod repo, and an advisory GitHub Actions check.

**Architecture:** A new `allod-substrate-git` crate implements the milestone-1 `Substrate` trait by shelling out to the `git` CLI (the existing `crates/allod/src/repo.rs` idiom — NOT gix; design doc amended in Task 7). Policy gains a parallel git evaluation path (`evaluate_git` over `GitChange`) plus a subject-parameterized reviewer-satisfaction helper extracted from `check_satisfied_with` with zero native behavior change. Decision records for commits live in **git notes** (`refs/notes/allod-decisions`) so deciding a commit never rewrites it — this replaces the design doc's decisions-in-`.allod/`-commits idea, which is circular for PR heads (committing the decision changes the SHA under decision).

**Tech Stack:** Rust 2021 workspace, serde_yaml, git CLI via `std::process::Command`, GitHub Actions.

## Global Constraints

- Zero behavior change to existing native paths: every pre-existing workspace test passes unmodified; `check_satisfied_with`'s native verdicts are byte-identical after the reviewer-helper extraction.
- Error idiom `Result<T, String>` in allod-core / allod-substrate-git; `AllodError` wrapping only at allod-graph/CLI call sites.
- Hash strings carry algorithm prefixes (§1.7): git commit revisions are `sha1:<40-hex>`, git tree state hashes are `sha1:<40-hex>`; decision subjects for commits are `git:<40-hex>` (bare SHA, `git:` scheme per §3.4).
- Determinism (§3.3): operation sets come from `git diff-tree` with rename detection disabled (`--no-renames`); two evaluators MUST derive identical operation sets.
- `allod-substrate-git` depends only on `allod-core`, `allod-substrate`, `serde_yaml`. It is never compiled to wasm and never a dependency of `allod-graph` or `allod-wasm`.
- Never commit private keys: `.allod/keys/` is gitignored before any graph lands in the repo.
- The CI check is advisory: a normal job that can fail, NOT marked required; no `continue-on-error`.
- Worktree: all work happens in the current worktree (branch `worktree-governed-review-m2`).

---

### Task 1: `glob_match` + `GitChange` + `evaluate_git` in policy.rs

The git-selector evaluation path. Native `selector_matches` keeps returning `false` for `substrate:` selectors (policy.rs:260-263 — unchanged); git changes are evaluated by a parallel entry point instead of being forced through `OpContext`.

**Files:**
- Modify: `crates/allod-core/src/policy.rs` (add below `evaluate`, around line 326)
- Test: same file's `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: existing `Checklist` (fields `matched_rules: BTreeSet<String>`, `reviewers: Vec<(String, u64)>`, `attestations: Vec<String>`, `root_required: bool`), `get_str`.
- Produces (Tasks 4 and 7 rely on these):
  - `pub fn glob_match(pattern: &str, text: &str) -> bool` — `*` matches any run of characters including `/`; everything else literal.
  - `pub struct GitChange { pub repo: String, pub target_ref: String, pub ops: Vec<(String, String)> }` — ops are `(verb, path)`, verb ∈ `create|update|delete`.
  - `pub fn evaluate_git(policy: &Value, change: &GitChange) -> Result<Checklist, String>`

Matching semantics (documented in the fn's doc comment): a rule participates only when its `select` is a bare map containing `substrate: git`. Change-level keys `repo` and `target_ref` glob-match the change's fields; op-level keys `path` (glob vs the op's path) and `operation` (string or list vs the op's verb) must match at least one op together. A rule with only change-level keys matches when the change has at least one op. Requirements union exactly as native `evaluate` does (reviewers dedup by `(role, quorum)`; `attestation_required.attester_class` collected). `root_required` is never set by the git path (default-posture fall-through is a native-substrate concept; advisory git evaluation reports only matched requirements — noted in the doc comment).

- [ ] **Step 1: Write the failing tests** (append inside policy.rs's existing test module):

```rust
// ---- git evaluation (§3.3 selectors) ----

fn git_policy() -> Value {
    serde_yaml::from_str(
        r#"
policy: repo-policy
version: 1
default_posture: restricted
roles:
  code-owner: [ "principal:conner" ]
  security:   [ "principal:conner" ]
rules:
  - name: main-requires-review
    select: { substrate: git, repo: "allod", target_ref: "refs/heads/main" }
    require:
      reviewers: { role: code-owner, quorum: 1 }
  - name: workflows-need-security
    select: { substrate: git, repo: "*", target_ref: "refs/heads/*", path: ".github/workflows/*" }
    require:
      reviewers: { role: security, quorum: 1 }
  - name: native-only-rule
    select: { type: "memory/Preference" }
    require:
      reviewers: { role: code-owner, quorum: 1 }
"#,
    )
    .unwrap()
}

#[test]
fn glob_match_star_and_literal() {
    assert!(glob_match("refs/heads/main", "refs/heads/main"));
    assert!(glob_match("refs/heads/*", "refs/heads/feat/x"));
    assert!(glob_match("*", "anything/at/all"));
    assert!(glob_match(".github/workflows/*", ".github/workflows/ci.yml"));
    assert!(!glob_match("refs/heads/main", "refs/heads/dev"));
    assert!(!glob_match(".github/workflows/*", "src/lib.rs"));
    assert!(glob_match("a*c*e", "abcde"));
    assert!(!glob_match("a*c*e", "abcdf"));
}

#[test]
fn evaluate_git_matches_ref_rule_and_skips_native_rules() {
    let change = GitChange {
        repo: "allod".into(),
        target_ref: "refs/heads/main".into(),
        ops: vec![("update".into(), "crates/allod-core/src/policy.rs".into())],
    };
    let cl = evaluate_git(&git_policy(), &change).unwrap();
    assert!(cl.matched_rules.contains("main-requires-review"));
    assert!(!cl.matched_rules.contains("native-only-rule"));
    assert_eq!(cl.reviewers, vec![("code-owner".to_string(), 1)]);
    assert!(!cl.root_required);
}

#[test]
fn evaluate_git_path_rule_needs_a_touching_op() {
    let policy = git_policy();
    let touches = GitChange {
        repo: "allod".into(),
        target_ref: "refs/heads/feat/x".into(),
        ops: vec![("create".into(), ".github/workflows/governance.yml".into())],
    };
    let cl = evaluate_git(&policy, &touches).unwrap();
    assert!(cl.matched_rules.contains("workflows-need-security"));

    let misses = GitChange {
        repo: "allod".into(),
        target_ref: "refs/heads/feat/x".into(),
        ops: vec![("update".into(), "README.md".into())],
    };
    let cl = evaluate_git(&policy, &misses).unwrap();
    assert!(!cl.matched_rules.contains("workflows-need-security"));
    // feat branch: main rule doesn't match either.
    assert!(cl.matched_rules.is_empty());
    assert!(cl.reviewers.is_empty());
}

#[test]
fn evaluate_git_unions_requirements_without_duplicates() {
    let change = GitChange {
        repo: "allod".into(),
        target_ref: "refs/heads/main".into(),
        ops: vec![
            ("update".into(), ".github/workflows/ci.yml".into()),
            ("update".into(), ".github/workflows/release-core.yml".into()),
        ],
    };
    let cl = evaluate_git(&git_policy(), &change).unwrap();
    assert!(cl.matched_rules.contains("main-requires-review"));
    assert!(cl.matched_rules.contains("workflows-need-security"));
    assert_eq!(
        cl.reviewers,
        vec![("code-owner".to_string(), 1), ("security".to_string(), 1)]
    );
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p allod-core evaluate_git`
Expected: FAIL to compile — `GitChange`, `evaluate_git`, `glob_match` not defined.

- [ ] **Step 3: Implement** (below `evaluate` in policy.rs):

```rust
// ---------------- git-substrate evaluation (§3.3) ----------------

/// Minimal glob: `*` matches any run of characters (including `/`),
/// everything else is literal. Rule patterns in §3.3 policies key on
/// repo, path, and branch shapes; this is the whole grammar.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    fn inner(p: &[u8], t: &[u8]) -> bool {
        match p.first() {
            None => t.is_empty(),
            Some(b'*') => {
                (0..=t.len()).any(|i| inner(&p[1..], &t[i..]))
            }
            Some(c) => t.first() == Some(c) && inner(&p[1..], &t[1..]),
        }
    }
    inner(pattern.as_bytes(), text.as_bytes())
}

/// A git changeset viewed for policy evaluation (§3.3): the repo name,
/// the ref the change targets, and the deterministic operation set as
/// (verb, path) pairs with verb in create|update|delete.
pub struct GitChange {
    pub repo: String,
    pub target_ref: String,
    pub ops: Vec<(String, String)>,
}

/// Evaluate a git change against policy rules whose `select` carries
/// `substrate: git` (§4.1, §3.3). Change-level keys `repo` and
/// `target_ref` glob-match the change; op-level keys `path` and
/// `operation` must co-match at least one op. Requirements union as in
/// `evaluate`. `root_required` is never set here: default-posture
/// fall-through is a native-substrate concept, and the advisory git
/// path reports matched requirements only.
pub fn evaluate_git(policy: &Value, change: &GitChange) -> Result<Checklist, String> {
    let rules = policy
        .get("rules")
        .and_then(Value::as_sequence)
        .ok_or("policy needs rules")?;
    let mut checklist = Checklist::default();
    for rule in rules {
        let (Some(name), Some(select), Some(require)) = (
            get_str(rule, "name"),
            rule.get("select"),
            rule.get("require"),
        ) else {
            continue;
        };
        if get_str(select, "substrate") != Some("git") {
            continue;
        }
        if let Some(repo_pat) = get_str(select, "repo") {
            if !glob_match(repo_pat, &change.repo) {
                continue;
            }
        }
        if let Some(ref_pat) = get_str(select, "target_ref") {
            if !glob_match(ref_pat, &change.target_ref) {
                continue;
            }
        }
        let path_pat = get_str(select, "path");
        let verbs: Option<Vec<&str>> = select.get("operation").map(|ops| match ops {
            Value::Sequence(seq) => seq.iter().filter_map(Value::as_str).collect(),
            other => other.as_str().into_iter().collect(),
        });
        let op_matches = |(verb, path): &(String, String)| -> bool {
            if let Some(pat) = path_pat {
                if !glob_match(pat, path) {
                    return false;
                }
            }
            if let Some(vs) = &verbs {
                if !vs.contains(&verb.as_str()) {
                    return false;
                }
            }
            true
        };
        if !change.ops.iter().any(op_matches) {
            continue;
        }
        checklist.matched_rules.insert(name.to_string());
        if let Some(reviewers) = require.get("reviewers") {
            let entries: Vec<&Value> = match reviewers {
                Value::Sequence(seq) => seq.iter().collect(),
                other => vec![other],
            };
            for entry in entries {
                if let Some(role) = get_str(entry, "role") {
                    let quorum = entry.get("quorum").and_then(Value::as_u64).unwrap_or(1);
                    let req = (role.to_string(), quorum);
                    if !checklist.reviewers.contains(&req) {
                        checklist.reviewers.push(req);
                    }
                }
            }
        }
        if let Some(att) = require.get("attestation_required") {
            if let Some(class) = get_str(att, "attester_class") {
                if !checklist.attestations.contains(&class.to_string()) {
                    checklist.attestations.push(class.to_string());
                }
            }
        }
    }
    Ok(checklist)
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p allod-core` then `cargo test --workspace`
Expected: PASS, no pre-existing test touched.

- [ ] **Step 5: Commit**

```bash
git add crates/allod-core/src/policy.rs
git commit -m "feat(policy): evaluate_git — §3.3 git-selector rules over (verb, path) operation sets"
```

---

### Task 2: extract subject-parameterized reviewer satisfaction

`check_satisfied_with` binds decision records by `subject == <native changeset hash>` inline (policy.rs:443-502). Extract the reviewer loop into a helper parameterized by subject so the git path (subject `git:<sha>`) reuses the same verification — signature over `decision_payload`, role bindings from policy, active-key check, quorum count. Zero native behavior change.

**Files:**
- Modify: `crates/allod-core/src/policy.rs:443-502` (the `for (role, quorum) in &checklist.reviewers` loop inside `check_satisfied_with`)
- Test: same file's test module

**Interfaces:**
- Produces: `pub fn reviewers_unmet(parent: &State, policy: &Value, subject: &str, checklist: &Checklist, decisions: &[Value]) -> Result<Vec<String>, String>` — returns the unmet strings (empty = satisfied). Computes `policy_context(policy)` internally and requires each record's `policy_context` to match, `subject` field to equal the `subject` argument, `verdict == "approve"`, decider principal bound to the role in `policy.roles`, and an active-key signature over `decision_payload(record)`. Unmet message format preserved verbatim: `"reviewers: role {role} needs quorum {quorum}, have {n}"`.
- `check_satisfied_with` calls `reviewers_unmet(parent, policy, &cs_hash, checklist, decisions)?` and extends its `unmet` with the result — the native subject stays the bare changeset hash exactly as today.

- [ ] **Step 1: Write the failing test** (append to policy.rs tests; build a minimal state with one principal — mirror how the existing `check_satisfied` tests in this file construct principals and signed decision records, reusing their helper fns; read the existing tests first and reuse their fixtures rather than inventing new ones):

```rust
#[test]
fn reviewers_unmet_binds_to_the_given_subject() {
    // Arrange exactly like the existing decision-record satisfaction test
    // in this module (same principal fixture, same signed record helper),
    // but sign a record whose subject is "git:abc123..." and policy_context
    // matches git_policy(). Assertions:
    //   - reviewers_unmet(.., subject = "git:abc123...", ..) with the record → empty vec
    //   - reviewers_unmet(.., subject = "git:OTHER", ..) with the same record
    //     → vec containing "reviewers: role code-owner needs quorum 1, have 0"
}
```

The arrange section adapts this module's existing fixtures (the assertions above are the contract). If no reusable signed-record helper exists, build one from `sign::Keypair::generate` + `decision_payload` following the pattern of whichever existing test signs a decision.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p allod-core reviewers_unmet`
Expected: FAIL to compile — `reviewers_unmet` not defined.

- [ ] **Step 3: Implement.** Move the loop body of `for (role, quorum) in &checklist.reviewers { … }` from `check_satisfied_with` into:

```rust
/// Check the reviewer requirements of a checklist against decision
/// records whose `subject` equals the given subject (§4.3 step 4).
/// Native admission passes the changeset hash; the git path (§3.3)
/// passes `git:<commit-sha>`. Returns unmet strings; empty means
/// satisfied.
pub fn reviewers_unmet(
    parent: &State,
    policy: &Value,
    subject: &str,
    checklist: &Checklist,
    decisions: &[Value],
) -> Result<Vec<String>, String> {
    let pctx = policy_context(policy)?;
    let mut unmet = Vec::new();
    // …the existing loop verbatim, with `cs_hash.as_str()` replaced by
    // `subject` and pushes going to this fn's `unmet`…
    Ok(unmet)
}
```

Keep every line of the moved loop otherwise identical (bindings lookup, `decision_payload`, decider iteration, active-key signature check, quorum comparison, message format). In `check_satisfied_with`, replace the loop with:

```rust
unmet.extend(reviewers_unmet(parent, policy, &cs_hash, checklist, decisions)?);
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p allod-core` then `cargo test --workspace`
Expected: PASS — every pre-existing satisfaction test green, unmodified.

- [ ] **Step 5: Commit**

```bash
git add crates/allod-core/src/policy.rs
git commit -m "refactor(policy): extract subject-parameterized reviewers_unmet; native behavior unchanged"
```

---

### Task 3: `allod-substrate-git` crate — GitSubstrate + decision notes

**Files:**
- Create: `crates/allod-substrate-git/Cargo.toml`
- Create: `crates/allod-substrate-git/src/lib.rs`
- Modify: root `Cargo.toml` (add `"crates/allod-substrate-git"` to members)
- Test: `crates/allod-substrate-git/tests/git_substrate.rs`

**Interfaces:**
- Consumes: `allod_substrate::{Substrate, Revision, AuthorVerdict}` (milestone 1 — `Revision { hash, parents, author: serde_yaml::Value, timestamp, signed }`), `allod_substrate::conformance::check_conformance`.
- Produces (Task 4 relies on):
  - `pub struct GitSubstrate { … }` with `pub fn new(repo_dir: &Path) -> GitSubstrate`
  - `impl Substrate for GitSubstrate` — revisions are `sha1:<40-hex>`; `operation_set` returns Values shaped `{ <verb>: { kind: file, id: <path>, blob: "sha1:<hex>", prior_blob: "sha1:<hex>" } }` (blob/prior_blob omitted when absent); verbs: `A`→create, `D`→delete, everything else (`M`, `T`, mode changes)→update
  - `pub fn op_paths(ops: &[serde_yaml::Value]) -> Vec<(String, String)>` — flatten to the `(verb, path)` pairs `policy::GitChange` consumes
  - `pub fn resolve_commit(repo_dir: &Path, commitish: &str) -> Result<String, String>` — full bare SHA via `git rev-parse --verify <commitish>^{commit}`
  - `pub fn read_decisions(repo_dir: &Path, sha: &str) -> Result<Vec<serde_yaml::Value>, String>` — parse the commit's `refs/notes/allod-decisions` note as YAML `{decisions: [...]}`; missing note → empty vec
  - `pub fn append_decision(repo_dir: &Path, sha: &str, record: &serde_yaml::Value) -> Result<(), String>` — read-modify-write the note (`git notes --ref=allod-decisions add -f`)

Implementation notes for the implementer (put the substance in doc comments, not just here):
- One private helper `fn git(repo: &Path, args: &[&str]) -> Result<String, String>` mirroring `crates/allod/src/repo.rs`'s (spawn `git -C <repo> <args>`, non-zero status → Err with stderr).
- `revision(hash)`: strip the `sha1:` prefix; `git rev-parse --verify <sha>^{commit}` must return the SAME sha (content addressing — a git object hash IS its content hash; a mismatch or unknown object is an Err). Then `git show -s --format=%P%n%aI%n%an <%ae>` for parents/timestamp/author, and `git cat-file commit <sha>` to detect a `gpgsig ` header (`signed`). `author` Value: mapping `{ name: …, email: … }`.
- `operation_set(hash)`: root commit (no parents) → `git diff-tree --no-renames --root -r --raw <sha>`; otherwise `git diff-tree --no-renames -r --raw <sha>` (which diffs against the first parent). Parse raw lines `":<oldmode> <newmode> <oldsha> <newsha> <status>\t<path>"`. Determinism per §3.3: no rename detection, byte-level, first parent.
- `state_hash(hash)`: `sha1:` + `git rev-parse <sha>^{tree}`.
- `heads()`: `git for-each-ref refs/heads --format=%(refname)%09%(objectname)` → `(refname, "sha1:"+sha)`.
- `verify_authorship(hash)`: unsigned commit → `Unsigned`. Signed → run `git verify-commit <sha>`; exit 0 → `Verified { principal: <author email>, key_id: String::new() }`, else `Failed(stderr)`. (Git-side key trust config — gpg keyrings or ssh allowed-signers — is deployment configuration; the substrate reports what git verifies.)

- [ ] **Step 1: Write the failing integration test** (`tests/git_substrate.rs`):

```rust
//! GitSubstrate against a real fixture repo, including the same §3.1
//! conformance suite the native substrate passes.

use allod_substrate::conformance::check_conformance;
use allod_substrate::{AuthorVerdict, Substrate};
use allod_substrate_git::{
    append_decision, op_paths, read_decisions, resolve_commit, GitSubstrate,
};
use std::path::{Path, PathBuf};
use std::process::Command;

fn sh(dir: &Path, cmd: &str, args: &[&str]) {
    let out = Command::new(cmd).arg("-C").arg(dir).args(args).output().unwrap();
    assert!(out.status.success(), "{cmd} {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// Build a two-commit repo: c1 adds src/lib.rs + README.md, c2 modifies
/// src/lib.rs and deletes README.md. Returns (dir, c1, c2) with sha1: prefixes.
fn fixture() -> (tempfile::TempDir, String, String) {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let git = |args: &[&str]| sh(d, "git", args);
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "test@allod.dev"]);
    git(&["config", "user.name", "Fixture"]);
    std::fs::create_dir_all(d.join("src")).unwrap();
    std::fs::write(d.join("src/lib.rs"), "pub fn one() {}\n").unwrap();
    std::fs::write(d.join("README.md"), "hello\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "c1"]);
    let c1 = rev(d, "HEAD");
    std::fs::write(d.join("src/lib.rs"), "pub fn one() {}\npub fn two() {}\n").unwrap();
    std::fs::remove_file(d.join("README.md")).unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "c2"]);
    let c2 = rev(d, "HEAD");
    (tmp, format!("sha1:{c1}"), format!("sha1:{c2}"))
}

fn rev(d: &Path, r: &str) -> String {
    let out = Command::new("git").arg("-C").arg(d).args(["rev-parse", r]).output().unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

#[test]
fn operation_set_is_deterministic_and_correct() {
    let (tmp, c1, c2) = fixture();
    let sub = GitSubstrate::new(tmp.path());

    let ops1 = sub.operation_set(&c1).unwrap();
    let pairs1 = op_paths(&ops1);
    assert!(pairs1.contains(&("create".to_string(), "src/lib.rs".to_string())));
    assert!(pairs1.contains(&("create".to_string(), "README.md".to_string())));

    let ops2 = sub.operation_set(&c2).unwrap();
    let pairs2 = op_paths(&ops2);
    assert_eq!(pairs2.len(), 2);
    assert!(pairs2.contains(&("update".to_string(), "src/lib.rs".to_string())));
    assert!(pairs2.contains(&("delete".to_string(), "README.md".to_string())));

    // Determinism: two computations agree byte-for-byte.
    assert_eq!(
        serde_yaml::to_string(&ops2).unwrap(),
        serde_yaml::to_string(&sub.operation_set(&c2).unwrap()).unwrap()
    );
}

#[test]
fn revision_parents_state_and_heads() {
    let (tmp, c1, c2) = fixture();
    let sub = GitSubstrate::new(tmp.path());

    let r2 = sub.revision(&c2).unwrap();
    assert_eq!(r2.hash, c2);
    assert_eq!(r2.parents, vec![c1.clone()]);
    assert!(r2.timestamp.is_some());
    assert!(!r2.signed, "fixture commits are unsigned");

    // Deterministic state: tree hash, stable across calls, differs across commits.
    assert_eq!(sub.state_hash(&c2).unwrap(), sub.state_hash(&c2).unwrap());
    assert_ne!(sub.state_hash(&c1).unwrap(), sub.state_hash(&c2).unwrap());

    let heads = sub.heads().unwrap();
    assert_eq!(heads, vec![("refs/heads/main".to_string(), c2.clone())]);

    // Unknown object is an error; authorship of unsigned commit is Unsigned.
    assert!(sub.revision("sha1:0000000000000000000000000000000000000000").is_err());
    assert!(matches!(sub.verify_authorship(&c2).unwrap(), AuthorVerdict::Unsigned));

    // resolve_commit expands a short/branch ref to the bare full sha.
    assert_eq!(format!("sha1:{}", resolve_commit(tmp.path(), "main").unwrap()), c2);
}

#[test]
fn conformance_all_but_authorship() {
    // check_conformance demands a Verified signed_rev; commit signing needs
    // key infrastructure the test host may lack. Run the generic suite and
    // accept exactly the authorship-precondition failure for an unsigned
    // fixture — every structural property must hold.
    let (tmp, _c1, c2) = fixture();
    let sub = GitSubstrate::new(tmp.path());
    match check_conformance(&sub, &c2) {
        Err(e) if e.contains("unsigned") => {} // fixture promised nothing more
        Err(e) => panic!("conformance failed structurally: {e}"),
        Ok(()) => panic!("unsigned fixture cannot satisfy the authorship property"),
    }
}

#[test]
fn decision_notes_round_trip_without_rewriting_the_commit() {
    let (tmp, _c1, c2) = fixture();
    let sha = c2.strip_prefix("sha1:").unwrap();

    assert!(read_decisions(tmp.path(), sha).unwrap().is_empty());

    let rec: serde_yaml::Value = serde_yaml::from_str(&format!(
        "{{ subject: 'git:{sha}', policy_context: 'sha256:p', verdict: approve, timestamp: '2026-08-04T00:00:00Z' }}"
    ))
    .unwrap();
    append_decision(tmp.path(), sha, &rec).unwrap();
    append_decision(tmp.path(), sha, &rec).unwrap(); // two records accumulate

    let got = read_decisions(tmp.path(), sha).unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(
        got[0].get("subject").and_then(|v| v.as_str()),
        Some(format!("git:{sha}").as_str())
    );

    // The decided commit is untouched: same sha still at HEAD.
    assert_eq!(rev(tmp.path(), "HEAD"), *sha);
}
```

Add `tempfile` as a dev-dependency (check the workspace for the version other crates use; if none uses it, latest 3.x).

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p allod-substrate-git`
Expected: FAIL — crate does not exist.

- [ ] **Step 3: Implement** `Cargo.toml` (deps: `allod-core`, `allod-substrate` by path, `serde_yaml` matching workspace version; dev-deps: `tempfile`), `src/lib.rs` per the Interfaces block above, and add the crate to workspace members. Doc comments cite §3.3 for the determinism rules and §3.4 for the `git:` subject scheme; module doc states why notes: decisions must attach to a commit without rewriting it.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p allod-substrate-git` then `cargo test --workspace`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/allod-substrate-git
git commit -m "feat(substrate-git): GitSubstrate over the git CLI; decisions in refs/notes/allod-decisions"
```

---

### Task 4: CLI — `allod git eval` and `allod git decide`

**Files:**
- Create: `crates/allod/src/gitcmd.rs`
- Modify: `crates/allod/src/main.rs` (add `mod gitcmd;`, dispatch arm, usage text)
- Modify: `crates/allod/Cargo.toml` (add `allod-substrate-git` path dependency; `allod-substrate` too if needed for the trait)
- Test: `crates/allod/tests/gitcmd.rs`

**Interfaces:**
- Consumes: `GitSubstrate::new`, `Substrate::operation_set`, `op_paths`, `resolve_commit`, `read_decisions`, `append_decision` (Task 3); `policy::{evaluate_git, GitChange, reviewers_unmet, policy_context, decision_payload}` (Tasks 1-2); `Graph::open`, `graph.policy()`, `graph.fold()`, `graph.load_key(name)` (existing store API); `allod_graph::ops::now_iso`.
- Produces: two subcommands (dispatched when the first positional is `git`):
  - `allod git eval <repo-dir> <commit-ish> --target <ref> [--graph <dir>] [--repo <name>] [--json]`
  - `allod git decide <repo-dir> <commit-ish> --as <principal> [--verdict approve|reject] [--graph <dir>]`

Behavior to implement (spell out in `gitcmd.rs` doc comments):
- Defaults: `--graph` = `<repo-dir>/.allod`; `--repo` = the repo dir's file name (`Path::file_name`); `--verdict` = `approve`.
- **eval**: resolve the commit; build `GitChange { repo, target_ref: <--target>, ops: op_paths(&operation_set) }`; `evaluate_git`; `read_decisions`; `reviewers_unmet(&state, &policy, &format!("git:{sha}"), &checklist, &decisions)`. Output human-readable by default (matched rules, each requirement with satisfied/unmet, verdict line); with `--json` print one JSON object `{"commit","target","matched_rules":[],"reviewers":[{"role","quorum","satisfied"}],"unmet":[],"verdict":"pass"|"fail"}` (serialize via `serde_yaml::Value` → `serde_json` is NOT in deps — construct the JSON string manually or via serde_yaml-to-JSON conversion if serde_json already appears in the crate's deps; check first, and if absent, hand-format with escaping only of the fixed fields, which are hashes/idents). Exit code 0 when `unmet` is empty, 1 otherwise (mirror how existing commands signal failure through `ExitCode` in main.rs).
- **decide**: build the record `{ subject: "git:<sha>", policy_context: policy_context(&policy)?, verdict, timestamp: now_iso(), deciders: [ { principal: "principal:<name>", key: <key_id>, signature: sign(decision_payload) } ] }` — mirror EXACTLY how the native decide flow in `crates/allod-graph/src/flows.rs` constructs and signs decision records (read it first; field names must match what `reviewers_unmet` verifies: `deciders[].principal`, `deciders[].signature`, subject/policy_context/verdict/timestamp under `decision_payload`). Sign with `graph.load_key(<principal name>)`. Append via `append_decision`. Print the subject and a confirmation.

- [ ] **Step 1: Write the failing integration test** (`crates/allod/tests/gitcmd.rs`) — drive the loop end to end through the library functions the commands call, OR through the binary with `std::process::Command` + `env!("CARGO_BIN_EXE_allod")` (preferred: tests the dispatch too):

```rust
//! End-to-end: init a governance graph, install a git policy, eval a
//! commit (fail), decide it, eval again (pass).

use std::path::Path;
use std::process::Command;

fn allod(args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_allod")).args(args).output().unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

fn sh(dir: &Path, args: &[&str]) {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn eval_decide_eval_loop_goes_green() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    sh(repo, &["init", "-q", "-b", "main"]);
    sh(repo, &["config", "user.email", "t@allod.dev"]);
    sh(repo, &["config", "user.name", "T"]);
    std::fs::write(repo.join("f.txt"), "x\n").unwrap();
    sh(repo, &["add", "."]);
    sh(repo, &["commit", "-q", "-m", "c1"]);

    // Governance graph beside the repo (schema dir from the workspace).
    let graph = repo.join(".allod");
    let schema = concat!(env!("CARGO_MANIFEST_DIR"), "/../../ontologies");
    let (ok, out) = allod(&["init", graph.to_str().unwrap(), "--owner", "conner", "--schema", schema]);
    assert!(ok, "init failed: {out}");

    // Install the repo policy (fixture file written by this test).
    let policy_yaml = r#"
policy: repo-policy
version: 1
default_posture: restricted
roles:
  code-owner: [ "principal:conner" ]
rules:
  - name: main-requires-review
    select: { substrate: git, repo: "*", target_ref: "refs/heads/main" }
    require:
      reviewers: { role: code-owner, quorum: 1 }
"#;
    let pfile = repo.join("policy.yaml");
    std::fs::write(&pfile, policy_yaml).unwrap();
    let (ok, out) = allod(&[
        "install-policy", graph.to_str().unwrap(), pfile.to_str().unwrap(), "--as", "conner",
    ]);
    assert!(ok, "install-policy failed: {out}");

    // Eval: held — no decision yet.
    let (ok, out) = allod(&[
        "git", "eval", repo.to_str().unwrap(), "HEAD", "--target", "refs/heads/main",
    ]);
    assert!(!ok, "eval must fail before any decision:\n{out}");
    assert!(out.contains("main-requires-review"), "checklist names the rule:\n{out}");
    assert!(out.contains("code-owner"), "unmet names the role:\n{out}");

    // Decide as the owner.
    let (ok, out) = allod(&[
        "git", "decide", repo.to_str().unwrap(), "HEAD", "--as", "conner",
    ]);
    assert!(ok, "decide failed: {out}");

    // Eval again: green.
    let (ok, out) = allod(&[
        "git", "eval", repo.to_str().unwrap(), "HEAD", "--target", "refs/heads/main", "--json",
    ]);
    assert!(ok, "eval must pass after the decision:\n{out}");
    assert!(out.contains("\"verdict\":\"pass\"") || out.contains("\"verdict\": \"pass\""), "{out}");

    // A feature ref matches no rule: trivially green.
    let (ok, _) = allod(&[
        "git", "eval", repo.to_str().unwrap(), "HEAD", "--target", "refs/heads/feat/x",
    ]);
    assert!(ok);
}
```

Note the test uses an `install-policy` CLI command. Check main.rs for an existing way to install a policy; if none exists, add `install-policy <graph-dir> <file> --as <principal>` as part of this task, delegating to the same flow the wasm `install_policy` binding uses (find it in `crates/allod-graph/src/flows.rs`; wire, don't reimplement). If that flow's admission holds the policy for approval instead of admitting (owner-root should self-admit under the memory profile), have the test approve the held proposal via the existing `approve` command — mirror how flows tests handle policy installation.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p allod --test gitcmd`
Expected: FAIL — `git` subcommand unknown (and possibly `install-policy`).

- [ ] **Step 3: Implement** `gitcmd.rs` with `pub fn cmd_git(args: &[String]) -> Result<(), String>` (or matching main.rs's existing command-fn signature convention — read a few `cmd_*` fns first and copy their shape), dispatching `eval`/`decide`, plus the main.rs wiring and usage lines. Add `tempfile` dev-dep to `crates/allod` if absent.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p allod` then `cargo test --workspace`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/allod
git commit -m "feat(cli): allod git eval / git decide — advisory checklist over commits, signed decisions in notes"
```

---

### Task 5: `review` ontology package

**Files:**
- Create: `ontologies/review/ontology.yaml`
- Create: `ontologies/review/README.md`

**Interfaces:**
- Consumes: the `core`, `code`, and `eng` packages (import pins: copy the `core` state_hash from `ontologies/code/ontology.yaml:11`, the `code` state_hash from `ontologies/eng/ontology.yaml:13`; obtain the `eng` hash the same way other packages that import eng do — search `ontologies/*/ontology.yaml` for an `ontology: eng` import to copy; if none exists, run `cargo run -p allod-lint -- ontologies` and use the hash it reports as expected).
- Produces: the data shapes milestone 4's freehold surface builds against.

- [ ] **Step 1: Write the package**

`ontologies/review/ontology.yaml`:

```yaml
# review — code-review artifacts, version 1 (projection form, §2.5)
#
# The §4.4 review artifact as typed graph data: a Review carries a
# verdict over a change; ReviewComments anchor prose to code through
# git: external refs. Comments ingested from an external host carry
# their origin in external_source/external_id and a claimed author —
# unsigned provenance, distinct from principal-signed reviews.

ontology: review
version: 1
imports:
  - { ontology: core, state_hash: "<copy from ontologies/code/ontology.yaml>" }
  - { ontology: code, state_hash: "<copy from ontologies/eng/ontology.yaml>" }
  - { ontology: eng,  state_hash: "<see Interfaces note>" }

entity_types:

  Review:
    attributes:
      verdict: { type: "enum<approve|approve-with-comments|request-changes>", required: true }
      body:    { type: string }                     # long text; the review prose
      commit:  { type: external-ref }               # git:<repo>#<sha> the review examined

  ReviewComment:
    attributes:
      body:            { type: string, required: true }
      anchor:          { type: external-ref }        # git:<repo>#<sha>:<path> plus span
      span:            { type: string }              # e.g. "L120-L184"
      status:          { type: "enum<open|resolved|wont-fix>" }
      external_source: { type: string }              # e.g. github
      external_id:     { type: string }              # host comment id, ingest dedup key
      claimed_author:  { type: string }              # unsigned external identity

edge_types:
  reviews:   { domain: Review, range: eng/ChangeRequest, cardinality: many-to-one }
  part_of:   { domain: ReviewComment, range: Review, cardinality: many-to-one }
  replies_to: { domain: ReviewComment, range: ReviewComment, cardinality: many-to-one }
  concerns:  { domain: ReviewComment, range: [code/SourceFile, code/Function], cardinality: many-to-many }

validation_rules:
  - name: external-comments-carry-their-id
    on:
      type: ReviewComment
      where: { attr: external_source, present: true }
    require: { attr: external_id, present: true }
```

`ontologies/review/README.md`: a short page in the style of `ontologies/code/README.md` — what the package models, that verdicts feed decision-record `basis` (§4.4), that external comments are claimed identities not signed authorship, and that anchors are content-addressed `git:` refs (§3.4). Follow the existing READMEs' register: plain declarative prose, mechanism only, no marketing.

- [ ] **Step 2: Lint**

Run: `cargo run -p allod-lint -- ontologies`
Expected: clean (fix import hashes or grammar complaints it raises; if lint validates enum/edge grammar differently than written, conform to what the existing packages do).

- [ ] **Step 3: Workspace check**

Run: `cargo test --workspace`
Expected: PASS (nothing consumes the package yet).

- [ ] **Step 4: Commit**

```bash
git add ontologies/review
git commit -m "feat(ontologies): review package — §4.4 review artifacts as typed graph data"
```

---

### Task 6: governance policy + genesis graph for the allod repo

**Files:**
- Create: `governance/policy.yaml`
- Create: `governance/README.md`
- Create: `scripts/init-governance.sh` (executable)
- Modify: `.gitignore` (add `.allod/keys/`)
- Create (generated, committed): `.allod/` — the genesis graph, WITHOUT `keys/`

**Interfaces:**
- Consumes: `allod init` + `install-policy` + `git eval`/`git decide` (Task 4).
- Produces: the live governance graph the CI workflow (Task 7) opens.

- [ ] **Step 1: Write the policy**

`governance/policy.yaml`:

```yaml
# repo-governance — the allod repository's own advisory review policy
# (projection form, §2.5). Evaluated by `allod git eval` in CI; the
# check is advisory (not a required status). Failing is the designed
# steady state until decisions land (design doc, CI section).
policy: repo-governance
version: 1
default_posture: restricted
roles:
  code-owner: [ "principal:conner" ]
rules:

  # Every change targeting main needs one code-owner decision (§3.3;
  # the CODEOWNERS + branch-protection shape, portable and signed).
  - name: main-requires-review
    select: { substrate: git, repo: "allod", target_ref: "refs/heads/main" }
    require:
      reviewers: { role: code-owner, quorum: 1 }

  # CI definitions are the gate's own substrate: changes to workflows
  # need review on every branch, not only main.
  - name: workflows-require-review
    select: { substrate: git, repo: "allod", target_ref: "refs/heads/*", path: ".github/workflows/*" }
    require:
      reviewers: { role: code-owner, quorum: 1 }

  # The governance graph and policy govern themselves: reviewed on main.
  - name: governance-requires-review
    select: { substrate: git, repo: "allod", target_ref: "refs/heads/main", path: "governance/*" }
    require:
      reviewers: { role: code-owner, quorum: 1 }
```

`governance/README.md`: what this directory is (the repo's own policy source), how genesis ran (`scripts/init-governance.sh`), where decisions live (`refs/notes/allod-decisions` — push with `git push origin refs/notes/allod-decisions`, fetch with `git fetch origin refs/notes/allod-decisions:refs/notes/allod-decisions`), and that `.allod/keys/` is gitignored — the signing key exists only on the owner's machine; CI verifies with the public keys recorded in graph state.

- [ ] **Step 2: Write the genesis script**

`scripts/init-governance.sh`:

```bash
#!/usr/bin/env bash
# Genesis for the allod repo's own governance graph (run once, by the
# owner, from the repo root). Creates .allod/ (keys stay local,
# gitignored) and installs governance/policy.yaml.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

if [ -d .allod ]; then
  echo ".allod already exists — refusing to re-run genesis" >&2
  exit 1
fi

cargo run -q -p allod -- init .allod --owner conner --schema ontologies
cargo run -q -p allod -- install-policy .allod governance/policy.yaml --as conner
cargo run -q -p allod -- verify .allod

echo
echo "Genesis complete. Commit .allod/ (keys/ is gitignored)."
```

`chmod +x scripts/init-governance.sh`. Add `.allod/keys/` to `.gitignore` FIRST (separate line, with a comment `# governance graph signing keys — never committed`).

- [ ] **Step 3: Run genesis and self-evaluate**

Run: `bash scripts/init-governance.sh`
Expected: init + policy install + verify all succeed.
Then dogfood once: `cargo run -q -p allod -- git eval . HEAD --target refs/heads/main` — expected: exit 1 with `main-requires-review` unmet (no decision for HEAD yet — correct advisory behavior). Then `cargo run -q -p allod -- git decide . HEAD --as conner` and re-eval — expected: pass. (The decision lands in local notes; the CI run on the eventual PR head will be red until a decision for THAT sha is pushed — that is the designed steady state.)

- [ ] **Step 4: Commit the graph (verify keys are excluded)**

```bash
git add .gitignore governance scripts/init-governance.sh .allod
git status --porcelain | grep -q "\.allod/keys" && { echo "KEYS STAGED — ABORT"; exit 1; } || true
git commit -m "feat(governance): genesis graph + repo policy; keys stay local"
```

Confirm with `git show --stat HEAD` that no `.allod/keys/` path appears.

---

### Task 7: CI workflow + design-doc amendments

**Files:**
- Create: `.github/workflows/governance.yml`
- Modify: `docs/superpowers/specs/2026-08-04-governed-code-review-design.md` (three amendments below)

- [ ] **Step 1: Write the workflow**

`.github/workflows/governance.yml`:

```yaml
# Advisory governance check (design doc: CLI and CI action). Evaluates
# the head commit against the repo's own governance graph. NOT a
# required status — red is the designed steady state until a decision
# for the evaluated sha lands in refs/notes/allod-decisions.
name: governance
on:
  pull_request:
  push:
    branches: [main]
jobs:
  eval:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      - name: Fetch decision notes
        run: git fetch origin '+refs/notes/allod-decisions:refs/notes/allod-decisions' || true
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Evaluate head against governance policy
        run: |
          TARGET="refs/heads/${{ github.base_ref || github.ref_name }}"
          SHA="${{ github.event.pull_request.head.sha || github.sha }}"
          echo "## Governance checklist" >> "$GITHUB_STEP_SUMMARY"
          echo '```' >> "$GITHUB_STEP_SUMMARY"
          set +e
          cargo run -q -p allod -- git eval . "$SHA" --target "$TARGET" --repo allod | tee -a "$GITHUB_STEP_SUMMARY"
          CODE=${PIPESTATUS[0]}
          set -e
          echo '```' >> "$GITHUB_STEP_SUMMARY"
          exit "$CODE"
```

(Check `ci.yml` for the toolchain/cache actions this repo already uses and match them instead of the ones above if they differ.)

- [ ] **Step 2: Amend the design doc** — three surgical edits:

1. In "### Substrate abstraction", change the `GitSubstrate` bullet's "(crate `allod-substrate-git`, gix-based)" to "(crate `allod-substrate-git`, over the git CLI — the `repo.rs` idiom; gix remains an option if shelling out ever binds)".
2. In "### Freehold surface" / data-flow prose, wherever decisions are said to land "into `.allod/`" for git changes, amend to: decisions for git commits live in git notes (`refs/notes/allod-decisions`), attached to the decided sha without rewriting it — committing a decision into the PR branch would change the head sha under decision. The governance graph still holds native-side records; freehold pushes the notes ref.
3. In "## Milestones", append to milestone 2's entry: " **Status: done.** Plan: `docs/superpowers/plans/2026-08-04-git-evaluation.md`. Deviations: git CLI not gix; decisions in git notes."

- [ ] **Step 3: Validate the workflow locally as far as possible**

Run: `cargo run -q -p allod -- git eval . HEAD --target refs/heads/main --repo allod; echo "exit: $?"`
Expected: runs end to end (exit 0 or 1 depending on whether HEAD has a decision — either is fine; what must not happen is a usage error or panic). Also `cargo test --workspace` one final time.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/governance.yml docs/superpowers/specs/2026-08-04-governed-code-review-design.md
git commit -m "feat(ci): advisory governance check; design doc records git-CLI and notes deviations"
```
