use allod_graph::ops::{self, Admission};

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
