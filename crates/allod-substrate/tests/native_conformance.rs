//! The native log passes the same §3.1 conformance suite the git
//! binding will run in milestone 2.

use allod_substrate::conformance::check_conformance;
use allod_substrate::native::NativeSubstrate;
use allod_substrate::Substrate;

/// Build a real in-memory graph with a signed changeset, returning
/// `(graph, tip_hash)`. Mirrors the setup pattern in
/// `crates/allod-graph/tests/flows.rs`.
fn fixture_graph() -> (allod_core::store::Graph, String) {
    use allod_core::docstore::MemStore;
    use allod_core::store::Graph;
    use std::path::PathBuf;

    let schema_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../ontologies");

    let store = Box::new(MemStore::new());
    let graph = Graph::with_store(store);

    let profile = allod_graph::flows::profile_from_dir("memory", &schema_dir)
        .expect("profile_from_dir");
    allod_graph::flows::init(&graph, "o", profile).expect("flows::init");

    // Add an agent so we have a signed non-genesis changeset.
    allod_graph::flows::principal_add(&graph, "agent1", "agent", "o")
        .expect("principal_add");

    // A note gives us an operation-bearing changeset.
    allod_graph::flows::note(&graph, "agent1", "fixture content")
        .expect("note");

    // The graph head is the most recent admitted changeset.
    let tip = graph.head().expect("head").expect("head exists after note");
    (graph, tip)
}

#[test]
fn native_substrate_conforms() {
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
