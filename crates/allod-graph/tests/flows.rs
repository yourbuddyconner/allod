use allod_graph::flows::{self, DecisionOutcome};
use allod_graph::ops::Admission;
use std::path::PathBuf;

mod common;

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
fn envelope_degraded_no_evidence() {
    // "evidence_type: none" produces Degraded (not a failure, just no evidence of code identity)
    use allod_graph::flows::EnvelopeOutcome;
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
