//! GitSubstrate against a real fixture repo, including the same §3.1
//! conformance suite the native substrate passes.

use allod_substrate::conformance::check_conformance;
use allod_substrate::{AuthorVerdict, Substrate};
use allod_substrate_git::{
    append_decision, op_paths, read_decisions, resolve_commit, GitSubstrate,
};
use std::path::Path;
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
    git(&["config", "commit.gpgsign", "false"]);
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

/// Build a repo with a no-ff merge commit: main has c1, feature branch adds
/// feature.txt, then merges into main with --no-ff. Returns (dir, feature_sha,
/// merge_sha) with sha1: prefixes.
fn fixture_with_merge() -> (tempfile::TempDir, String, String) {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let git = |args: &[&str]| sh(d, "git", args);
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "test@allod.dev"]);
    git(&["config", "user.name", "Fixture"]);
    git(&["config", "commit.gpgsign", "false"]);

    // Initial commit on main.
    std::fs::write(d.join("base.txt"), "base\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "c1 base"]);

    // Create and switch to feature branch, add feature.txt.
    git(&["checkout", "-q", "-b", "feature"]);
    std::fs::write(d.join("feature.txt"), "feature content\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "add feature.txt"]);
    let feature_sha = rev(d, "HEAD");

    // Merge feature into main with --no-ff to produce a merge commit.
    git(&["checkout", "-q", "main"]);
    git(&["merge", "--no-ff", "-q", "-m", "Merge feature branch", "feature"]);
    let merge_sha = rev(d, "HEAD");

    (tmp, format!("sha1:{feature_sha}"), format!("sha1:{merge_sha}"))
}

#[test]
fn merge_commit_operation_set_equals_first_parent_diff() {
    let (tmp, _feature_sha, merge_sha) = fixture_with_merge();
    let sub = GitSubstrate::new(tmp.path());

    // The merge commit's operation set must reflect what the merged branch
    // contributed (feature.txt added) — not be empty.
    let merge_ops = sub.operation_set(&merge_sha).unwrap();
    let merge_pairs = op_paths(&merge_ops);

    assert!(
        !merge_pairs.is_empty(),
        "merge commit operation set must not be empty"
    );
    assert!(
        merge_pairs.contains(&("create".to_string(), "feature.txt".to_string())),
        "merge commit must show feature.txt as created; got: {merge_pairs:?}"
    );

    // Must equal the first-parent diff (only feature.txt changed relative to main).
    assert_eq!(merge_pairs.len(), 1, "only feature.txt was added by the merge");

    // Determinism: two calls return identical serialized output.
    let merge_ops2 = sub.operation_set(&merge_sha).unwrap();
    assert_eq!(
        serde_yaml::to_string(&merge_ops).unwrap(),
        serde_yaml::to_string(&merge_ops2).unwrap(),
        "merge operation set must be deterministic"
    );
}
