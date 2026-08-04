//! Test helpers: in-memory graph setup, no printing.

use allod_core::docstore::MemStore;
use allod_core::fold::State;
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

fn read_yaml(path: PathBuf) -> Value {
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

/// Port of main.rs cmd_init_profile (memory profile) against MemStore.
pub fn init_memory_graph() -> Graph {
    let store = Box::new(MemStore::new());
    let graph = Graph::with_store(store);
    let owner = "o";
    let kp = Keypair::generate(owner);
    graph.save_key(&kp).expect("save_key");

    let schema = schema_dir();
    for (name, rel) in &[
        ("core", "core/ontology.yaml"),
        ("memory", "memory/ontology.yaml"),
        ("memory-taxonomy", "memory/taxonomy.yaml"),
    ] {
        graph
            .install_schema(name, &read_yaml(schema.join(rel)))
            .unwrap_or_else(|e| panic!("install_schema {name}: {e}"));
    }

    let mut policy = read_yaml(schema.join("memory/policy-local.yaml"));
    if let Some(roles) = policy.get_mut("roles").and_then(Value::as_mapping_mut) {
        let bind = Value::Sequence(vec![s(&format!("principal:{owner}"))]);
        let names: Vec<Value> = roles.keys().cloned().collect();
        for name in names {
            roles.insert(name, bind.clone());
        }
    }
    graph.install_schema("policy", &policy).expect("install policy");

    // Genesis: build changeset manually (same as main.rs:260-285).
    let owner_node = ops::uuid4();
    let mut attrs = Mapping::new();
    attrs.insert(s("display_name"), s(owner));
    attrs.insert(s("keys"), Value::Sequence(vec![ops::key_record(&kp)]));
    attrs.insert(s("status"), s("active"));
    let node_op = ops::create_node_op(&owner_node, "core/User@1", Value::Mapping(attrs), None);

    let (cs, hash) = ops::build_changeset(
        &graph,
        &kp,
        &format!("Genesis: root authority {owner}, core + memory schema, memory-local policy"),
        vec![node_op],
    )
    .expect("build genesis changeset");

    let reg = graph.registry().expect("registry");
    let mut state = State::default();
    state.apply_changeset(&reg, &cs).expect("apply genesis");
    graph.append_changeset(&cs, &hash, None).expect("append genesis");
    graph
        .write_meta(&hash, &[format!("principal:{owner}")])
        .expect("write_meta");
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
