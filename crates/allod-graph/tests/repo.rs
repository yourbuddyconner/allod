//! Integration tests for the repo import module (§8.3).
//!
//! Guard: requires the `native` feature (spawns git, uses the filesystem).
//! Exercises make_sample_repo → import_commit (idempotent) → semantic_diff.

#![cfg(feature = "native")]

use allod_core::docstore::MemStore;
use allod_core::store::Graph;
use allod_graph::repo;
use std::path::PathBuf;
use tempfile::TempDir;

mod common;

fn schema_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ontologies")
}

fn init_code_graph() -> Graph {
    let store = Box::new(MemStore::new());
    let graph = Graph::with_store(store);
    let profile = allod_graph::flows::profile_from_dir("code", &schema_dir())
        .expect("profile_from_dir code");
    allod_graph::flows::init(&graph, "owner", profile).expect("flows::init code");
    allod_graph::flows::principal_add(&graph, "indexer", "service", "owner")
        .expect("add indexer");
    graph
}

/// make_sample_repo creates a git repo with two commits in the given dir.
#[test]
fn make_sample_repo_returns_two_shas() {
    let tmp = TempDir::new().expect("tmpdir");
    let repo_dir = tmp.path().join("repo");
    let (first, second) = repo::make_sample_repo(&repo_dir).expect("make_sample_repo");
    assert!(!first.is_empty(), "first commit sha should not be empty");
    assert!(!second.is_empty(), "second commit sha should not be empty");
    assert_ne!(first, second, "two commits should have different shas");
}

/// import_commit derives graph nodes for a commit and admits them.
#[test]
fn import_commit_derives_nodes() {
    let tmp = TempDir::new().expect("tmpdir");
    let repo_dir = tmp.path().join("repo");
    let (first, _second) = repo::make_sample_repo(&repo_dir).expect("make_sample_repo");

    let graph = init_code_graph();
    let (hash, admitted) = repo::import_commit(&graph, &repo_dir, &first, "indexer")
        .expect("import_commit");
    assert!(!hash.is_empty(), "changeset hash should not be empty");
    assert!(admitted, "first import should be admitted");

    // Graph should now contain the authorize_spend function node.
    let state = graph.fold().expect("fold");
    let has_fn = state.objects.iter().any(|((kind, _id), obj)| {
        kind == "node"
            && allod_core::get_str(&obj.content, "type").map(allod_core::bare)
                == Some("code/Function")
            && obj
                .content
                .get("attributes")
                .and_then(|a| allod_core::get_str(a, "name"))
                == Some("authorize_spend")
    });
    assert!(has_fn, "authorize_spend function node should be derived");
}

/// A second import_commit of the same commit produces no ops (idempotent).
#[test]
fn import_commit_same_commit_is_idempotent() {
    let tmp = TempDir::new().expect("tmpdir");
    let repo_dir = tmp.path().join("repo");
    let (first, _second) = repo::make_sample_repo(&repo_dir).expect("make_sample_repo");

    let graph = init_code_graph();
    repo::import_commit(&graph, &repo_dir, &first, "indexer")
        .expect("first import_commit");

    // Second import of the same commit returns "nothing changed" error.
    let result = repo::import_commit(&graph, &repo_dir, &first, "indexer");
    match result {
        Err(e) => assert!(
            e.to_string().contains("nothing changed"),
            "expected idempotent error, got: {e}"
        ),
        Ok((_, admitted)) => {
            // If the implementation supersedes instead of errors, it must not admit new ops.
            assert!(!admitted, "second import of same commit should not admit anything");
        }
    }
}

/// semantic_diff returns a typed struct describing changed functions.
#[test]
fn semantic_diff_returns_changed_functions() {
    let tmp = TempDir::new().expect("tmpdir");
    let repo_dir = tmp.path().join("repo");
    let (first, second) = repo::make_sample_repo(&repo_dir).expect("make_sample_repo");

    let graph = init_code_graph();
    repo::import_commit(&graph, &repo_dir, &first, "indexer")
        .expect("import first commit");
    let (h2, _) = repo::import_commit(&graph, &repo_dir, &second, "indexer")
        .expect("import second commit");

    let diff = repo::semantic_diff(&graph, &h2).expect("semantic_diff");
    // The second commit updates authorize_spend and adds audit/refund.
    assert!(
        !diff.entries.is_empty(),
        "semantic diff should have at least one function entry"
    );
    let names: Vec<&str> = diff.entries.iter().map(|e| e.name.as_str()).collect();
    assert!(
        names.contains(&"authorize_spend"),
        "authorize_spend should appear in diff entries; got: {names:?}"
    );
    // semantic_diff should not print anything — callers render it
}
