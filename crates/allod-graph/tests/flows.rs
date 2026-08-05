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

/// Pinning test: verify report verdicts must be identical before and after the
/// substrate rewire (Task 5). Run green first, stay green after.
#[test]
fn verify_report_is_stable_across_substrate_rewire() {
    use allod_graph::flows::LevelResult;
    use allod_core::store::Graph as CoreGraph;
    use allod_graph::flows;
    use allod_core::get_str;
    use tempfile::TempDir;

    // ── Part 1: clean graph ──
    // Every changeset must report integrity=Verified and authorship=Verified.
    {
        let graph = common::init_memory_graph();
        flows::principal_add(&graph, "agent", "agent", "o").unwrap();
        flows::note(&graph, "agent", "pinning content").unwrap();
        let prop = flows::propose_preference(&graph, "agent", "prefer pinning", "soft", None).unwrap();
        flows::decide(&graph, &prop.hash, "o", "approve").unwrap();

        let report = flows::verify(&graph).expect("verify clean graph");
        assert!(report.ok, "clean graph must verify ok");
        for cs_entry in &report.changesets {
            assert!(
                matches!(cs_entry.integrity, LevelResult::Verified),
                "clean graph: integrity not Verified for {}", cs_entry.hash
            );
            assert!(
                matches!(cs_entry.authorship, LevelResult::Verified),
                "clean graph: authorship not Verified for {}", cs_entry.hash
            );
        }
    }

    // ── Part 2: corrupted changeset → integrity failure ──
    // We use a filesystem-backed graph so we can overwrite the changeset YAML.
    //
    // Current behavior (pre-rewire): graph.registry() folds the whole chain
    // and fails fast when it detects the hash mismatch, so flows::verify returns
    // Err rather than a VerifyReport. After the substrate rewire, verify uses
    // sub.revision() per-changeset in the loop, so it returns Ok(report) with
    // integrity=Failed for the tampered entry.
    //
    // This test accepts both behaviors so it passes before AND after the rewire.
    {
        let tmp = TempDir::new().expect("tempdir");
        let graph_dir = tmp.path();
        // Graph::create(dir) sets up FsStore at dir/.allod
        let graph = CoreGraph::create(graph_dir).expect("create fs graph");
        let profile = flows::profile_from_dir("memory",
            &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ontologies"))
            .expect("profile_from_dir");
        flows::init(&graph, "o", profile).expect("init");
        flows::principal_add(&graph, "agent", "agent", "o").unwrap();
        flows::note(&graph, "agent", "some note for pinning").unwrap();

        // Identify the last changeset (the note).
        let chain = graph.chain().expect("chain");
        let last_cs = chain.last().expect("at least one cs");
        let last_hash = get_str(last_cs, "hash").expect("hash").to_string();
        let short = last_hash.strip_prefix("sha256:").unwrap_or(&last_hash);

        // Overwrite the changeset file to break its content address.
        // FsStore roots at graph_dir/.allod so changesets live there.
        let cs_path = graph_dir.join(".allod").join("changesets").join(format!("{short}.yaml"));
        let original = std::fs::read_to_string(&cs_path).expect("read changeset file");
        // Change the timestamp field without touching the hash field.
        // changeset_hash() includes the timestamp in its preimage, so when
        // flows::verify re-reads the chain the recomputed hash won't match
        // the stored hash field → apply_changeset returns Err → integrity=Failed.
        let tampered = original.replace("timestamp:", "timestamp: TAMPERED #");
        assert_ne!(tampered, original, "tamper must change the file — no 'timestamp:' in changeset?");
        std::fs::write(&cs_path, &tampered).expect("write tampered changeset");

        // Verify must detect the corruption. Pre-rewire: verify returns Err (fold
        // fails fast). Post-rewire: verify returns Ok(report) with integrity=Failed.
        match flows::verify(&graph) {
            Err(_) => {
                // Pre-rewire behavior: fold failure propagates as Err. Acceptable.
            }
            Ok(report) => {
                // Post-rewire behavior: integrity failure captured in the report.
                assert!(!report.ok, "tampered graph must not verify ok");
                let failed_entry = report.changesets.iter()
                    .find(|cs| cs.hash == last_hash)
                    .expect("tampered changeset appears in report");
                assert!(
                    matches!(failed_entry.integrity, LevelResult::Failed(_)),
                    "tampered changeset must have integrity=Failed, got: {:?}",
                    std::mem::discriminant(&failed_entry.integrity)
                );
            }
        }
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

// ---- install_package ----

/// Helper: return the op count of a changeset that is either admitted (in the log) or
/// held (in proposals), identified by `hash`.
fn op_count_for(graph: &allod_core::store::Graph, result: &Admission) -> usize {
    use allod_core::get_str;
    match result {
        Admission::Admitted { hash, .. } => {
            // Find the changeset in the log by hash.
            let chain = graph.chain().expect("chain");
            chain
                .iter()
                .find(|cs| get_str(cs, "hash") == Some(hash.as_str()))
                .and_then(|cs| cs.get("operations").and_then(serde_yaml::Value::as_sequence))
                .map(|ops| ops.len())
                .unwrap_or(0)
        }
        Admission::Held { hash, .. } => {
            // Find the changeset in proposals by hash.
            let cs = graph.read_proposal(hash).expect("read_proposal");
            cs.get("operations")
                .and_then(serde_yaml::Value::as_sequence)
                .map(|ops| ops.len())
                .unwrap_or(0)
        }
    }
}

#[test]
fn install_package_with_policy_emits_one_policy_op() {
    let graph = common::init_memory_graph();
    // Get the genesis changeset to count initial log entries
    let initial_log = flows::log(&graph).expect("log");
    assert!(!initial_log.is_empty(), "genesis changeset should exist");

    // Create a minimal schema doc with one entity type so ops are emitted
    let schema_doc = serde_yaml::from_str(
        "ontology: test\nentity_types:\n  TestType:\n    attributes:\n      name: {type: string}\n"
    ).expect("schema doc");
    let docs = vec![("test".to_string(), schema_doc)];

    // Create a policy (copy from genesis)
    let policy = allod_graph::flows::profile_from_dir("memory", &schema_dir())
        .expect("profile_from_dir")
        .policy;

    let result = flows::install_package(&graph, &docs, Some(&policy), "o")
        .expect("install_package");
    // Note: install_package calls ops::commit which applies policy governance.
    // With a policy that requires set-policy approval, the changeset may be Held.
    let is_admitted = matches!(result, Admission::Admitted { .. });
    let is_held = matches!(result, Admission::Held { .. });
    assert!(is_admitted || is_held, "should be admitted or held, got {:?}", result);

    // Verify the changeset has at least EntityType + Policy ops regardless of hold/admit.
    let count = op_count_for(&graph, &result);
    assert!(count >= 2, "should have at least EntityType + Policy ops, got {count}");
}

#[test]
fn install_package_without_policy_emits_no_policy_op() {
    let graph = common::init_memory_graph();

    // Create a schema doc with one entity type
    let schema_doc = serde_yaml::from_str(
        "ontology: test\nentity_types:\n  TestType:\n    attributes:\n      name: {type: string}\n"
    ).expect("schema doc");
    let docs = vec![("test".to_string(), schema_doc)];

    let result = flows::install_package(&graph, &docs, None, "o")
        .expect("install_package");
    // Note: install_package calls ops::commit which applies policy governance.
    let is_admitted = matches!(result, Admission::Admitted { .. });
    let is_held = matches!(result, Admission::Held { .. });
    assert!(is_admitted || is_held, "should be admitted or held, got {:?}", result);

    // With policy=None, the changeset must have exactly one op (EntityType only, no Policy).
    let count = op_count_for(&graph, &result);
    assert_eq!(count, 1, "should have exactly one op (EntityType only, no Policy) when policy is None, got {count}");
}

// ---- install_package: policy deduplication ----

/// When install_package is called with Some(policy) and a live meta/Policy node
/// already exists, the function must rewrite the create op as an update op so
/// the graph ends up with exactly one meta/Policy node instead of erroring.
#[test]
fn install_package_updates_existing_policy() {
    let graph = common::init_memory_graph();

    // After init, a meta/Policy node already exists (installed during genesis).
    // Verify by checking that graph.policy() succeeds.
    graph.policy().expect("genesis policy should exist");

    // Build a minimal schema doc
    let schema_doc: serde_yaml::Value = serde_yaml::from_str(
        "ontology: test2\nentity_types:\n  AnotherType:\n    attributes:\n      label: {type: string}\n"
    ).expect("schema doc");
    let docs = vec![("test2".to_string(), schema_doc)];

    // Get the original policy and tweak it slightly (add a comment-like field won't work in
    // YAML Value, so just reuse the same policy — the important thing is it goes through
    // the update path).
    let original_policy = allod_graph::flows::profile_from_dir("memory", &schema_dir())
        .expect("profile_from_dir")
        .policy;

    // This should NOT error with "create of existing object".
    let result = flows::install_package(&graph, &docs, Some(&original_policy), "o")
        .expect("install_package with existing policy should succeed via update op");

    let is_admitted = matches!(result, Admission::Admitted { .. });
    let is_held = matches!(result, Admission::Held { .. });
    assert!(is_admitted || is_held, "should be admitted or held, got {:?}", result);

    // The changeset must contain the expected ops (at least EntityType + updated Policy).
    let count = op_count_for(&graph, &result);
    assert!(count >= 2, "should have at least EntityType + Policy update ops, got {count}");

    // If admitted, the graph state must reflect exactly one meta/Policy node.
    // If held (proposal queue), the state is unchanged from genesis — policy still exists.
    // Either way, graph.policy() must succeed (no "multiple meta/Policy" error).
    graph.policy().expect("exactly one meta/Policy node should exist");

    // Registry must also load cleanly (would error if multiple-policy collision).
    graph.registry().expect("registry should load without multiple-policy error");
}

// ---- genesis sentinel contract ----

#[test]
fn genesis_schema_context_equals_genesis_constant() {
    use allod_core::model::GENESIS_SCHEMA_CONTEXT;

    let graph = common::init_memory_graph();
    let chain = graph.chain().expect("chain");
    assert!(!chain.is_empty(), "genesis changeset should exist");

    let genesis = &chain[0];
    let schema_context = allod_core::get_str(genesis, "schema_context")
        .expect("genesis changeset has schema_context field");

    assert_eq!(schema_context, GENESIS_SCHEMA_CONTEXT,
        "genesis changeset's schema_context must equal GENESIS_SCHEMA_CONTEXT");
}

#[test]
fn second_changeset_schema_context_differs_from_genesis() {
    use allod_core::model::GENESIS_SCHEMA_CONTEXT;

    let graph = common::init_memory_graph();
    flows::principal_add(&graph, "a1", "agent", "o").expect("principal_add");

    let chain = graph.chain().expect("chain");
    assert!(chain.len() >= 2, "should have at least 2 changesets");

    let genesis = &chain[0];
    let genesis_context = allod_core::get_str(genesis, "schema_context")
        .expect("genesis has schema_context");
    assert_eq!(genesis_context, GENESIS_SCHEMA_CONTEXT, "genesis context must be constant");

    let second = &chain[1];
    let second_context = allod_core::get_str(second, "schema_context")
        .expect("second changeset has schema_context");

    assert_ne!(second_context, GENESIS_SCHEMA_CONTEXT,
        "second changeset's schema_context must differ from genesis constant");
}

#[test]
fn verify_reports_ok_on_graph_with_schema() {
    let graph = common::init_memory_graph();
    flows::principal_add(&graph, "a1", "agent", "o").expect("principal_add");

    let report = flows::verify(&graph).expect("verify");
    assert!(report.ok, "verify should report ok after init and principal_add");
}

// ---- EC2: entity type governance end-to-end flow ----

#[test]
fn ec2_entity_type_governance_flow() {
    use allod_graph::flows::{self, DecisionOutcome};
    use allod_graph::ops::{self, Admission};

    // 1. Init graph with memory profile
    let graph = common::init_memory_graph();

    // 2. Add agent principal
    flows::principal_add(&graph, "worker", "agent", "o").expect("add worker agent");

    // 3. Agent proposes install_package with a new entity type — should be Held
    // because "worker" is an agent and schema changes require owner quorum.
    let schema_doc: serde_yaml::Value = serde_yaml::from_str(
        "ontology: memory\nentity_types:\n  TaskItem:\n    attributes:\n      title: {type: string, required: true}\n      status: {type: string}\n"
    ).expect("schema doc");
    let docs = vec![("memory".to_string(), schema_doc)];

    let proposal = flows::install_package(&graph, &docs, None, "worker")
        .expect("install_package proposal");
    let proposal_hash = match &proposal {
        Admission::Held { hash, .. } => hash.clone(),
        Admission::Admitted { .. } => panic!("schema proposal should be Held, not Admitted"),
    };

    // 4. Owner approves → Admitted
    let outcome = flows::decide(&graph, &proposal_hash, "o", "approve")
        .expect("decide");
    assert!(
        matches!(outcome, DecisionOutcome::Admitted { .. }),
        "owner approval should admit schema changeset, got: {:?}", std::mem::discriminant(&outcome)
    );

    // 5. Owner creates an instance of the new type using ops::commit directly
    let mut title_attrs = serde_yaml::Mapping::new();
    title_attrs.insert(
        serde_yaml::Value::String("title".to_string()),
        serde_yaml::Value::String("First task".to_string()),
    );
    title_attrs.insert(
        serde_yaml::Value::String("status".to_string()),
        serde_yaml::Value::String("open".to_string()),
    );
    let task_node_op = ops::create_node_op(
        &ops::uuid4(),
        "memory/TaskItem@1",
        serde_yaml::Value::Mapping(title_attrs),
        None,
    );
    let _instance_admission = ops::commit(
        &graph,
        "o",
        "Create TaskItem instance",
        vec![task_node_op],
        vec![],
    ).expect("owner creates TaskItem instance");

    // 6. verify reports ok
    let report = flows::verify(&graph).expect("verify");
    let failed_hashes: Vec<&str> = report.changesets.iter()
        .filter(|e| !matches!(e.integrity, allod_graph::flows::LevelResult::Verified))
        .map(|e| e.hash.as_str())
        .collect();
    assert!(report.ok, "verify should be ok after EC2 flow; failed changesets: {:?}", failed_hashes);
}
