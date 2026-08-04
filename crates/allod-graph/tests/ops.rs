use allod_graph::flows::{self, DecisionOutcome};
use allod_graph::ops::{self, Admission};
use serde_yaml::Value;

mod common;

#[test]
fn scratch_note_admits_and_preference_holds() {
    let graph = common::init_memory_graph();
    common::add_agent(&graph, "jarvis", "o");

    let note_id = ops::uuid4();
    let note_ops = vec![
        ops::create_node_op(
            &note_id,
            "memory/Note@1",
            serde_yaml::from_str("content: prefers tea").unwrap(),
            Some(common::provenance("jarvis")),
        ),
        ops::classification_op(
            &format!("node:{note_id}"),
            "workspace/scratch@1",
            "principal:jarvis",
            "model-assisted",
        ),
    ];
    match ops::commit(&graph, "jarvis", "Scratch note", note_ops, vec![]).unwrap() {
        Admission::Admitted { .. } => {}
        other => panic!("scratch should admit, got {other:?}"),
    }

    let pref_id = ops::uuid4();
    let pref_ops = vec![
        ops::create_node_op(
            &pref_id,
            "memory/Preference@1",
            serde_yaml::from_str("statement: tea over coffee\nstrength: strong").unwrap(),
            Some(common::provenance("jarvis")),
        ),
        ops::classification_op(
            &format!("node:{pref_id}"),
            "work@1",
            "principal:jarvis",
            "model-assisted",
        ),
    ];
    match ops::commit(&graph, "jarvis", "Propose preference", pref_ops, vec![]).unwrap() {
        Admission::Held { checklist, .. } => {
            assert!(!checklist.matched_rules.is_empty());
        }
        other => panic!("preference should hold, got {other:?}"),
    }
}

#[test]
fn commit_with_envelope_then_approve_is_admitted() {
    let graph = common::init_memory_graph();
    flows::principal_add(&graph, "agent", "agent", "o").unwrap();

    // Build preference ops manually (mirrors what createEntity does in Freehold)
    let pref_id = ops::uuid4();
    let mut attrs = serde_yaml::Mapping::new();
    attrs.insert(Value::String("statement".into()), Value::String("prefers dark mode".into()));
    attrs.insert(Value::String("strength".into()), Value::String("soft".into()));
    let mut prov = serde_yaml::Mapping::new();
    prov.insert(Value::String("derived_by".into()), Value::String("principal:agent".into()));
    prov.insert(Value::String("method".into()), Value::String("model-assisted".into()));
    prov.insert(Value::String("tool".into()), Value::String("freehold@0.1".into()));

    let node_op = ops::create_node_op(
        &pref_id,
        "memory/Preference@1",
        Value::Mapping(attrs),
        Some(Value::Mapping(prov)),
    );

    // commit_with_envelope builds + signs envelope → should be Held (needs owner approve)
    let admission = ops::commit_with_envelope(
        &graph,
        "agent",
        "Create memory/Preference@1",
        vec![node_op],
    )
    .unwrap();

    let hash = match &admission {
        Admission::Held { hash, .. } => hash.clone(),
        Admission::Admitted { .. } => panic!("expected Held, got Admitted"),
    };

    // Owner approves — with signed envelope the model-assisted-needs-signed-envelope rule is met
    let outcome = flows::decide(&graph, &hash, "o", "approve").unwrap();
    assert!(
        matches!(outcome, DecisionOutcome::Admitted { .. }),
        "expected Admitted after approve, got: {outcome:?}"
    );
}
