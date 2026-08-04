use allod_graph::flows::{self, DecisionOutcome};
use allod_graph::ops::Admission;
use std::path::PathBuf;

mod common;

// ---- proposals ----

#[test]
fn proposals_lists_pending() {
    let graph = common::init_memory_graph();
    flows::principal_add(&graph, "p1", "agent", "o").unwrap();
    // No proposals initially
    let empty = flows::proposals(&graph).expect("proposals");
    assert!(empty.is_empty());
    // Create a proposal
    let r = flows::propose_preference(&graph, "p1", "prefer tea", "soft", None).unwrap();
    assert!(matches!(r.admission, Admission::Held { .. }));
    let list = flows::proposals(&graph).expect("proposals");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].intent, "Propose preference: prefer tea");
}

// ---- log ----

#[test]
fn log_lists_changesets() {
    let graph = common::init_memory_graph();
    let entries = flows::log(&graph).expect("log");
    // After init, at least 1 changeset (genesis)
    assert!(!entries.is_empty());
    assert!(!entries[0].hash.is_empty());
    assert!(!entries[0].author.is_empty());
    assert!(entries[0].op_count > 0);
}

// ---- state ----

#[test]
fn state_returns_nodes_and_hash() {
    let graph = common::init_memory_graph();
    flows::principal_add(&graph, "a6", "agent", "o").unwrap();
    flows::note(&graph, "a6", "some content").unwrap();
    let view = flows::state(&graph).expect("state");
    assert!(!view.state_hash.is_empty());
    // There should be at least the owner node and the agent node
    assert!(view.nodes.len() >= 2);
    // Owner node has a label
    let has_owner = view.nodes.iter().any(|n| n.label == "o");
    assert!(has_owner, "owner node 'o' should be in state");
}

// ---- verify ----

#[test]
fn verify_full_jarvis_flow() {
    use allod_graph::flows::LevelResult;
    let graph = common::init_memory_graph();
    flows::principal_add(&graph, "agent", "agent", "o").unwrap();
    let note_r = flows::note(&graph, "agent", "some content").unwrap();
    let prop = flows::propose_preference(&graph, "agent", "prefer foo", "soft", Some(&note_r.note_id)).unwrap();
    flows::decide(&graph, &prop.hash, "o", "approve").unwrap();
    flows::checkpoint(&graph, "o").unwrap();

    let report = flows::verify(&graph).expect("verify");
    assert!(report.ok, "verify should be ok");
    assert!(!report.changesets.is_empty());
    assert!(!report.state_hash.is_empty());
    assert!(!report.checkpoints.is_empty());
    // All changesets should have integrity Verified
    for cs_entry in &report.changesets {
        assert!(matches!(cs_entry.integrity, LevelResult::Verified), "integrity failed for {}", cs_entry.hash);
        assert!(matches!(cs_entry.authorship, LevelResult::Verified), "authorship failed for {}", cs_entry.hash);
        assert!(matches!(cs_entry.governance, LevelResult::Verified), "governance failed for {}", cs_entry.hash);
    }
    // All checkpoints should pass
    for cp in &report.checkpoints {
        assert!(cp.ok, "checkpoint {} failed", cp.revision);
    }
}

#[test]
fn verify_governance_failure_has_real_reason() {
    use allod_graph::flows::LevelResult;

    // Build the full jarvis flow to get an admitted preference changeset.
    let graph = common::init_memory_graph();
    flows::principal_add(&graph, "agent", "agent", "o").unwrap();
    let note_r = flows::note(&graph, "agent", "test content").unwrap();
    let prop = flows::propose_preference(&graph, "agent", "prefer foo", "soft", Some(&note_r.note_id)).unwrap();
    flows::decide(&graph, &prop.hash, "o", "approve").unwrap();

    // Find the preference changeset hash (the most recently admitted, non-genesis one
    // that would be held under normal policy — it was admitted by a decision record).
    let chain = graph.chain().expect("chain");
    let pref_cs_hash = allod_core::get_str(chain.last().unwrap(), "hash")
        .unwrap()
        .to_string();

    // Overwrite that changeset's evidence with an empty decisions/envelopes doc.
    // This makes verify re-evaluate governance and find the requirements unmet.
    let empty_evidence: serde_yaml::Value = serde_yaml::from_str(
        "decisions: []\nenvelopes: []\n"
    ).unwrap();
    graph.write_evidence(&pref_cs_hash, &empty_evidence).expect("write_evidence");

    // Verify: governance for the corrupted changeset must fail with a real reason.
    let report = flows::verify(&graph).expect("verify returns a report even on failure");
    assert!(!report.ok, "verify must not be ok after evidence erasure");

    let failed_entry = report.changesets.iter()
        .find(|cs| cs.hash == pref_cs_hash)
        .expect("corrupted changeset appears in report");
    match &failed_entry.governance {
        LevelResult::Failed(reason) => {
            assert_ne!(reason, "not reached",
                "governance failure must contain real reason, not sentinel");
            // The reason must name the unmet requirement(s).
            assert!(
                reason.contains("governance FAILS") || reason.contains("unmet"),
                "reason should name the unmet requirement, got: {reason}"
            );
        }
        other => panic!("expected governance Failed, got {:?}", std::mem::discriminant(other)),
    }
}

// ---- Task 4b stubs (will be implemented one by one) ----

// ---- trust ----

#[test]
fn trust_measurement_ok() {
    let graph = common::init_memory_graph();
    let measurement = allod_core::hash::plain_sha256(b"test-tool-v1");
    flows::trust(&graph, &measurement).expect("trust");
    // Verify the measurement is stored
    let measurements = graph.trusted_measurements().expect("trusted_measurements");
    assert!(measurements.iter().any(|m| m == &measurement));
}

// ---- envelope ----

#[test]
fn envelope_verified_with_trusted_measurement() {
    use allod_graph::flows::EnvelopeOutcome;
    let graph = common::init_memory_graph();
    // Get the head changeset hash
    let cs_hash = graph.head().expect("head").expect("some head");
    // Trust the tool measurement
    let tool = "test-scan-tool@1.0";
    let measurement = allod_core::hash::plain_sha256(tool.as_bytes());
    flows::trust(&graph, &measurement).expect("trust");
    // Build and verify an envelope
    let outcome = flows::envelope(&graph, &cs_hash, "o", tool).expect("envelope");
    assert!(matches!(outcome, EnvelopeOutcome::Verified(_)));
}

#[test]
fn envelope_err_untrusted_measurement() {
    // EnvelopeOutcome::Degraded is currently unreachable via flows::envelope (it always
    // builds simulated evidence and errors on untrusted measurements rather than degrading).
    // We test the untrusted-measurement error path here instead.
    // We can't exercise the "none" evidence_type path through the public flows::envelope API
    // (it always builds simulated evidence). But we can verify the policy directly,
    // and separately verify that an untrusted simulated measurement becomes Err.
    let graph = common::init_memory_graph();
    let cs_hash = graph.head().expect("head").expect("some head");
    // Untrusted measurement → Err (Failed)
    let tool = "untrusted-tool@1.0";
    let result = flows::envelope(&graph, &cs_hash, "o", tool);
    assert!(result.is_err(), "untrusted measurement must be Err");
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("not in the trusted set") || msg.contains("envelope failed"), "unexpected: {msg}");
}

fn schema_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ontologies")
}

// ---- Task 1a: profile_from_dir + init ----

#[test]
fn init_creates_graph_with_owner() {
    use allod_core::docstore::MemStore;
    use allod_core::store::Graph;
    let graph = Graph::with_store(Box::new(MemStore::new()));
    let profile = flows::profile_from_dir("memory", &schema_dir()).expect("profile_from_dir");
    let result = flows::init(&graph, "alice", profile).expect("init");
    assert_eq!(result.owner, "alice");
    assert!(!result.graph_id.is_empty());
}

// ---- Task 1b: principal_add ----

#[test]
fn principal_add_agent_admits() {
    let graph = common::init_memory_graph();
    let result = flows::principal_add(&graph, "bot", "agent", "o").expect("principal_add");
    assert!(!result.node_id.is_empty());
    assert!(matches!(result.admission, Admission::Admitted { .. }));
}

// ---- Task 1c: note ----

#[test]
fn note_scratch_admits() {
    let graph = common::init_memory_graph();
    flows::principal_add(&graph, "a1", "agent", "o").unwrap();
    let result = flows::note(&graph, "a1", "hello world").expect("note");
    assert!(!result.note_id.is_empty());
    assert!(matches!(result.admission, Admission::Admitted { .. }));
}

// ---- Task 1d: propose_preference ----

#[test]
fn propose_preference_holds() {
    let graph = common::init_memory_graph();
    flows::principal_add(&graph, "a2", "agent", "o").unwrap();
    let result = flows::propose_preference(&graph, "a2", "prefer tea", "soft", None).expect("propose");
    assert!(!result.hash.is_empty());
    assert!(matches!(result.admission, Admission::Held { .. }));
}

// ---- Task 1e: decide ----

#[test]
fn decide_approve_admits() {
    let graph = common::init_memory_graph();
    flows::principal_add(&graph, "a3", "agent", "o").unwrap();
    let r = flows::propose_preference(&graph, "a3", "prefer quiet", "soft", None).unwrap();
    let outcome = flows::decide(&graph, &r.hash, "o", "approve").expect("decide");
    assert!(matches!(outcome, DecisionOutcome::Admitted { .. }));
}

#[test]
fn decide_reject_stays_auditable() {
    let graph = common::init_memory_graph();
    flows::principal_add(&graph, "a4", "agent", "o").unwrap();
    let r = flows::propose_preference(&graph, "a4", "prefer dark mode", "soft", None).unwrap();
    let outcome = flows::decide(&graph, &r.hash, "o", "reject").expect("decide");
    assert!(matches!(outcome, DecisionOutcome::Rejected));
}

// ---- Task 1f: classify ----

#[test]
fn classify_by_owner_admits() {
    let graph = common::init_memory_graph();
    flows::principal_add(&graph, "a5", "agent", "o").unwrap();
    let note_r = flows::note(&graph, "a5", "test content").unwrap();
    let admission = flows::classify(&graph, &note_r.note_id, "workspace/scratch@1", "o", "manual")
        .expect("classify");
    assert!(matches!(admission, Admission::Admitted { .. }));
}

// ---- Task 1g: checkpoint ----

#[test]
fn checkpoint_records_state() {
    let graph = common::init_memory_graph();
    let result = flows::checkpoint(&graph, "o").expect("checkpoint");
    assert!(!result.revision.is_empty());
    assert!(!result.state_hash.is_empty());
}
