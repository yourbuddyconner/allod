//! Tests for registry introspection (schema.rs) and embedded profiles (profiles.rs).

use std::path::PathBuf;

mod common;

fn ontologies_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ontologies")
}

// ---- schema::describe tests ----

#[test]
fn describe_lists_memory_note_with_content_attribute() {
    let graph = common::init_memory_graph();
    let desc = allod_graph::schema::describe(&graph).expect("describe");

    // memory/Note must appear as an entity type
    let note = desc
        .entity_types
        .iter()
        .find(|e| e.name == "memory/Note")
        .expect("memory/Note not found in entity_types");

    // Note must have a 'content' attribute
    let content_attr = note
        .attributes
        .iter()
        .find(|a| a.name == "content")
        .expect("content attribute not found on memory/Note");

    assert_eq!(content_attr.type_expr, "string");
    assert!(content_attr.required);
}

#[test]
fn describe_lists_workspace_scratch_term_with_workspace_parent() {
    let graph = common::init_memory_graph();
    let desc = allod_graph::schema::describe(&graph).expect("describe");

    let scratch = desc
        .terms
        .iter()
        .find(|t| t.name == "workspace/scratch")
        .expect("workspace/scratch not found in terms");

    assert!(
        scratch.parents.contains(&"workspace".to_string()),
        "workspace/scratch should have parent 'workspace', got: {:?}",
        scratch.parents
    );
}

#[test]
fn describe_includes_edge_types() {
    let graph = common::init_memory_graph();
    let desc = allod_graph::schema::describe(&graph).expect("describe");
    // memory ontology has 'relates_to' and 'about' edge types
    let names: Vec<&str> = desc.edge_types.iter().map(|e| e.name.as_str()).collect();
    assert!(
        names.iter().any(|n| n.contains("relates_to")),
        "expected relates_to in edge types, got: {names:?}"
    );
}

// ---- profiles::embedded_profile tests ----

#[test]
fn embedded_profile_memory_equals_profile_from_dir() {
    let embedded = allod_graph::profiles::embedded_profile("memory")
        .expect("embedded_profile(memory)");
    let from_dir =
        allod_graph::flows::profile_from_dir("memory", &ontologies_dir())
            .expect("profile_from_dir(memory)");

    assert_eq!(embedded.name, from_dir.name);
    assert_eq!(
        embedded.docs.len(),
        from_dir.docs.len(),
        "doc count mismatch"
    );
    for ((ename, edoc), (dname, ddoc)) in
        embedded.docs.iter().zip(from_dir.docs.iter())
    {
        assert_eq!(ename, dname, "doc name mismatch");
        assert_eq!(edoc, ddoc, "doc content mismatch for {ename}");
    }
    assert_eq!(embedded.policy, from_dir.policy, "policy mismatch");
}

#[test]
fn embedded_profile_code_equals_profile_from_dir() {
    let embedded = allod_graph::profiles::embedded_profile("code")
        .expect("embedded_profile(code)");
    let from_dir =
        allod_graph::flows::profile_from_dir("code", &ontologies_dir())
            .expect("profile_from_dir(code)");

    assert_eq!(embedded.name, from_dir.name);
    assert_eq!(embedded.docs.len(), from_dir.docs.len());
    for ((ename, edoc), (dname, ddoc)) in
        embedded.docs.iter().zip(from_dir.docs.iter())
    {
        assert_eq!(ename, dname);
        assert_eq!(edoc, ddoc, "doc content mismatch for {ename}");
    }
    assert_eq!(embedded.policy, from_dir.policy);
}

#[test]
fn embedded_profile_unknown_name_is_err() {
    let result = allod_graph::profiles::embedded_profile("nope");
    assert!(result.is_err(), "expected Err for unknown profile name");
    let err = result.err().expect("is_err checked above");
    let msg = err.to_string();
    assert!(msg.contains("nope"), "error message should mention the name: {msg}");
}
