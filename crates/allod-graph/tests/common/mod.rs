//! Test helpers: in-memory graph setup, no printing.

use allod_core::docstore::MemStore;
use allod_core::sign::Keypair;
use allod_core::store::Graph;
use allod_graph::ops;
use serde_yaml::{Mapping, Value};
use std::path::PathBuf;

fn s(v: &str) -> Value {
    Value::String(v.to_string())
}

fn schema_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ontologies")
}

pub fn init_memory_graph() -> Graph {
    let store = Box::new(MemStore::new());
    let graph = Graph::with_store(store);
    let profile = allod_graph::flows::profile_from_dir("memory", &schema_dir())
        .expect("profile_from_dir");
    allod_graph::flows::init(&graph, "o", profile).expect("flows::init");
    graph
}

/// Port of main.rs cmd_principal_add for kind "agent".
pub fn add_agent(graph: &Graph, name: &str, by: &str) {
    let owner_kp = graph.load_key(by).expect("load owner key");
    let kp = Keypair::generate(name);
    graph.save_key(&kp).expect("save agent key");

    let state = graph.fold().expect("fold");
    let (_, owner_obj) = state
        .find_principal(&format!("principal:{by}"))
        .unwrap_or_else(|| panic!("unknown principal {by}"));
    let owner_node = allod_core::get_str(&owner_obj.content, "id")
        .unwrap_or("")
        .to_string();

    let mut attrs = Mapping::new();
    attrs.insert(s("display_name"), s(name));
    attrs.insert(s("keys"), Value::Sequence(vec![ops::key_record(&kp)]));
    attrs.insert(s("status"), s("active"));
    attrs.insert(s("delegated_by"), s(&format!("node:{owner_node}")));
    attrs.insert(
        s("scope"),
        serde_yaml::from_str("{ region: workspace }").unwrap(),
    );

    let node_op = ops::create_node_op(&ops::uuid4(), "core/Agent@1", Value::Mapping(attrs), None);

    let (cs, hash) = ops::build_changeset(
        graph,
        &owner_kp,
        &format!("Register agent {name}, by {by}"),
        vec![node_op],
    )
    .expect("build agent changeset");

    ops::admit_or_hold(graph, by, &cs, &hash, vec![]).expect("admit agent");
}

/// Build a provenance value for the given agent.
pub fn provenance(agent: &str) -> Value {
    let mut prov = Mapping::new();
    prov.insert(s("derived_by"), s(&format!("principal:{agent}")));
    prov.insert(s("method"), s("model-assisted"));
    prov.insert(s("tool"), s("test-helper"));
    Value::Mapping(prov)
}
