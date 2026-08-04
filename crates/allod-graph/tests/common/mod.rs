//! Test helpers: in-memory graph setup, no printing.
#![allow(dead_code)]

use allod_core::docstore::MemStore;
use allod_core::store::Graph;
use serde_yaml::{Mapping, Value};
use std::path::PathBuf;

fn s(v: &str) -> Value {
    Value::String(v.to_string())
}

fn schema_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ontologies")
}

pub fn init_memory_graph() -> Graph {
    init_memory_graph_owner("o")
}

/// Initialize a MemStore memory graph with a custom owner name.
pub fn init_memory_graph_owner(owner: &str) -> Graph {
    let store = Box::new(MemStore::new());
    let graph = Graph::with_store(store);
    let profile = allod_graph::flows::profile_from_dir("memory", &schema_dir())
        .expect("profile_from_dir");
    allod_graph::flows::init(&graph, owner, profile).expect("flows::init");
    graph
}

/// Register an agent principal via flows::principal_add.
pub fn add_agent(graph: &Graph, name: &str, by: &str) {
    allod_graph::flows::principal_add(graph, name, "agent", by)
        .expect("add_agent via flows::principal_add");
}

/// Build a provenance value for the given agent.
pub fn provenance(agent: &str) -> Value {
    let mut prov = Mapping::new();
    prov.insert(s("derived_by"), s(&format!("principal:{agent}")));
    prov.insert(s("method"), s("model-assisted"));
    prov.insert(s("tool"), s("test-helper"));
    Value::Mapping(prov)
}
