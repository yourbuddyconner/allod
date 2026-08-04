//! Integration tests for the federation module (§9 / Part 9).
//!
//! Two MemStore graphs: A grants a region to B, `make_bundle`, B imports
//! under its policy; asserts the imported object's lineage carries the
//! `allod:` reference; revoke; asserts second `make_bundle` fails.

use allod_graph::fed;
use allod_graph::ops::Admission;

mod common;

/// Helper: register B's graph-id as a peer in graph A under principal `by`.
fn peer_add(graph: &allod_core::store::Graph, name: &str, peer_graph_id: &str, peer_key: &str, by: &str) {
    fed::peer_add(graph, name, peer_graph_id, peer_key, by).expect("peer_add");
}

#[test]
fn federation_grant_bundle_import_revoke() {
    // ---- Graph A: owner "conner", agent "jarvis" ----
    let graph_a = common::init_memory_graph();
    common::add_agent(&graph_a, "jarvis", "o");

    // Jarvis writes a note; conner promotes it to a Preference (work region).
    let note_r = allod_graph::flows::note(&graph_a, "jarvis", "No meetings before 09:00")
        .expect("note");
    let prop_r = allod_graph::flows::propose_preference(
        &graph_a,
        "jarvis",
        "No meetings before 09:00",
        "soft",
        Some(&note_r.note_id),
    )
    .expect("propose_preference");
    allod_graph::flows::decide(&graph_a, &prop_r.hash, "o", "approve")
        .expect("decide approve");

    // ---- Graph B: owner "dana" ----
    let graph_b = common::init_memory_graph_owner("dana");

    // B registers A as a peer.
    let a_meta = graph_a.meta().expect("graph_a meta");
    let a_graph_id = allod_core::get_str(&a_meta, "graph_id").expect("graph_id").to_string();
    let a_key = graph_a.load_key("o").expect("load_key o").public_hex();
    peer_add(&graph_b, "conner-memory", &a_graph_id, &a_key, "dana");

    let b_meta = graph_b.meta().expect("graph_b meta");
    let b_graph_id = allod_core::get_str(&b_meta, "graph_id").expect("graph_id").to_string();

    // A grants region "work" to B.
    let grant_id = fed::grant(&graph_a, &b_graph_id, "work", "o").expect("grant");
    assert!(!grant_id.is_empty());

    // A produces a share bundle (pure Value, no filesystem).
    let bundle = fed::make_bundle(&graph_a, &grant_id, "o").expect("make_bundle");

    // Find the preference node ID in A's state.
    let state_a = graph_a.fold().expect("fold A");
    let pref_id = state_a
        .objects
        .iter()
        .find_map(|((kind, id), obj)| {
            (kind == "node"
                && allod_core::get_str(&obj.content, "type")
                    .map(allod_core::bare)
                    == Some("memory/Preference"))
            .then(|| id.clone())
        })
        .expect("preference node in A");

    // B imports the preference.
    let admissions = fed::import(&graph_b, &bundle, "dana", &pref_id)
        .expect("import");

    // The import might be held or admitted (policy-dependent). Either way the
    // call succeeds (no Err) and we get exactly one Admission for the object.
    assert_eq!(admissions.len(), 1, "expected one Admission from import");

    // The import may be held; if so, dana approves it.
    match &admissions[0] {
        Admission::Held { hash, .. } => {
            allod_graph::flows::decide(&graph_b, &hash, "dana", "approve")
                .expect("dana approve held import");
        }
        Admission::Admitted { .. } => {}
    }

    // Verify the imported object in B carries the allod: lineage reference.
    let state_b = graph_b.fold().expect("fold B");
    let imported_node = state_b
        .objects
        .iter()
        .find_map(|((kind, _), obj)| {
            if kind != "node" || obj.deleted {
                return None;
            }
            if allod_core::get_str(&obj.content, "type")
                .map(allod_core::bare)
                == Some("memory/Preference")
            {
                Some(obj.content.clone())
            } else {
                None
            }
        })
        .expect("imported preference node in B");

    // provenance.derived_from should contain "allod:<a_graph_id>/<pref_id>@<rev>"
    let prov = imported_node
        .get("provenance")
        .expect("provenance field on imported node");
    let derived_from = prov
        .get("derived_from")
        .and_then(|v| v.as_sequence())
        .expect("derived_from sequence");
    assert_eq!(derived_from.len(), 1);
    let ref_str = derived_from[0].as_str().expect("derived_from[0] is string");
    assert!(
        ref_str.starts_with(&format!("allod:{a_graph_id}/{pref_id}@")),
        "lineage ref should be allod:<graph>/<id>@<rev>, got: {ref_str}"
    );

    // ---- Revocation: after revoke, make_bundle fails ----
    fed::revoke(&graph_a, &grant_id, "o").expect("revoke");
    let err = fed::make_bundle(&graph_a, &grant_id, "o")
        .expect_err("make_bundle should fail after revocation");
    assert!(
        err.to_string().contains("revoked") || err.to_string().contains("no live grant"),
        "expected revocation error, got: {err}"
    );
}
