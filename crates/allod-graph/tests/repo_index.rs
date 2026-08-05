//! Indexer extension (§8.3): all-language file granularity, deletion
//! handling, .allod/ exclusion.

#![cfg(feature = "native")]

use allod_core::get_str;
use allod_graph::repo::import_commit;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

mod common;

fn sh(dir: &Path, args: &[&str]) {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn fixture_graph() -> (allod_core::store::Graph, String) {
    use allod_core::docstore::MemStore;
    use allod_core::store::Graph;
    use std::path::PathBuf;
    let store = Box::new(MemStore::new());
    let graph = Graph::with_store(store);
    let schema_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ontologies");
    let profile = allod_graph::flows::profile_from_dir("code", &schema_dir)
        .expect("profile_from_dir code");
    allod_graph::flows::init(&graph, "owner", profile).expect("flows::init");
    allod_graph::flows::principal_add(&graph, "owner", "service", "owner")
        .expect("add owner as service");
    (graph, "owner".to_string())
}

fn git_fixture() -> TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let git = |args: &[&str]| sh(d, args);
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "test@allod.dev"]);
    git(&["config", "user.name", "Fixture"]);
    git(&["config", "commit.gpgsign", "false"]);
    tmp
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

#[test]
fn deleting_a_file_removes_inbound_call_edges_from_survivors() {
    let (graph, _owner) = fixture_graph();
    let repo = git_fixture();
    std::fs::create_dir_all(repo.path().join("src")).unwrap();
    // a.rs declares f (and g calling f); b.rs declares h which calls f cross-file.
    std::fs::write(repo.path().join("src/a.rs"), "pub fn f() {}\npub fn g() { f() }\n").unwrap();
    std::fs::write(repo.path().join("src/b.rs"), "pub fn h() { f() }\n").unwrap();
    sh(repo.path(), &["add", "-A"]);
    sh(repo.path(), &["commit", "-q", "-m", "c1"]);
    import_commit(&graph, repo.path(), "HEAD", "owner").unwrap();

    // c2: delete a.rs entirely; b.rs survives (h keeps its text).
    std::fs::remove_file(repo.path().join("src/a.rs")).unwrap();
    sh(repo.path(), &["add", "-A"]);
    sh(repo.path(), &["commit", "-q", "-m", "c2"]);
    import_commit(&graph, repo.path(), "HEAD", "owner").unwrap();

    let state = graph.fold().unwrap();
    let live_fn = |n: &str| {
        state.objects.iter().any(|((k, _), o)| {
            k == "node"
                && !o.deleted
                && get_str(&o.content, "type").map(allod_core::bare) == Some("code/Function")
                && o.content.get("attributes").and_then(|a| get_str(a, "name")) == Some(n)
        })
    };
    assert!(!live_fn("f") && !live_fn("g"), "a.rs items tombstoned");
    assert!(live_fn("h"), "survivor h intact");
    for ((k, id), o) in &state.objects {
        if k == "edge" && !o.deleted {
            for side in ["from", "to"] {
                let r = get_str(&o.content, side).unwrap();
                assert!(state.resolve_ref(r).is_some(), "edge {id} dangling {side}");
            }
        }
    }
}
