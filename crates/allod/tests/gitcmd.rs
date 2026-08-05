//! End-to-end: init a governance graph, install a git policy, eval a
//! commit (fail), decide it, eval again (pass).

use std::path::Path;
use std::process::Command;

fn test_keys_dir() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("allod-gitcmd-keys-{}", std::process::id()))
}

fn allod(args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_allod"))
        .args(args)
        .env("ALLOD_KEYS_DIR", test_keys_dir())
        .output().unwrap();
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

    // Governance graph at the repo root (schema dir from the workspace).
    let schema = concat!(env!("CARGO_MANIFEST_DIR"), "/../../ontologies");
    let (ok, out) = allod(&["init", repo.to_str().unwrap(), "--owner", "conner", "--schema", schema]);
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
        "install-policy", repo.to_str().unwrap(), pfile.to_str().unwrap(), "--as", "conner",
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

#[test]
fn git_index_materializes_and_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    sh(repo, &["init", "-q", "-b", "main"]);
    sh(repo, &["config", "user.email", "t@allod.dev"]);
    sh(repo, &["config", "user.name", "T"]);

    // Create a Rust source file to be indexed.
    std::fs::write(repo.join("main.rs"), "fn foo() {}\n").unwrap();
    sh(repo, &["add", "."]);
    sh(repo, &["commit", "-q", "-m", "c1"]);

    // Initialize with code profile to support repository/sourcefile/function indexing.
    let schema = concat!(env!("CARGO_MANIFEST_DIR"), "/../../ontologies");
    let (ok, out) = allod(&[
        "init-code",
        repo.to_str().unwrap(),
        "--owner",
        "conner",
        "--schema",
        schema,
    ]);
    assert!(ok, "init-code failed: {out}");

    // 1. First index: should succeed and print admission
    let (ok, out) = allod(&[
        "git", "index", repo.to_str().unwrap(), "HEAD", "--as", "conner",
    ]);
    assert!(ok, "first index failed: {out}");
    assert!(
        out.contains("admitted") || out.contains("held"),
        "admission outcome not found in: {out}"
    );

    // 2. Second index: should succeed and print "up to date"
    let (ok, out) = allod(&[
        "git", "index", repo.to_str().unwrap(), "HEAD", "--as", "conner",
    ]);
    assert!(ok, "second index failed: {out}");
    assert!(
        out.contains("up to date"),
        "up to date message not found in: {out}"
    );
}
