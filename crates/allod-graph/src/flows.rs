//! High-level command flows: bodies of the CLI commands, printing removed.

use allod_core::store::Graph;
use std::path::Path;
use serde_yaml::Value;

use crate::AllodError;
use crate::ops::Admission;

// ---- Result types ----

pub struct ProfileSource {
    pub name: String,
    pub docs: Vec<(String, Value)>,
    pub policy: Value,
}

pub struct InitResult {
    pub graph_id: String,
    pub owner: String,
}

pub struct PrincipalAdded {
    pub node_id: String,
    pub admission: Admission,
}

pub struct NoteResult {
    pub note_id: String,
    pub admission: Admission,
}

pub struct ProposalResult {
    pub hash: String,
    pub admission: Admission,
}

#[derive(serde::Serialize)]
pub enum DecisionOutcome {
    Rejected,
    StillUnmet { unmet: Vec<String> },
    Admitted { degraded: Vec<String> },
}

pub struct CheckpointResult {
    pub revision: String,
    pub state_hash: String,
}

// ---- Flow functions (stubs — replaced one by one) ----

/// Load schema docs and policy from an on-disk schema directory for the given profile.
/// Supported profiles: "memory", "code".
pub fn profile_from_dir(profile: &str, schema_dir: &Path) -> Result<ProfileSource, AllodError> {
    let read = |p: std::path::PathBuf| -> Result<Value, AllodError> {
        let text = std::fs::read_to_string(&p)
            .map_err(|e| AllodError::Other(format!("{}: {e}", p.display())))?;
        serde_yaml::from_str(&text)
            .map_err(|e| AllodError::Other(e.to_string()))
    };

    let (files, policy_path): (Vec<(&str, &str)>, &str) = match profile {
        "memory" => (
            vec![
                ("core", "core/ontology.yaml"),
                ("memory", "memory/ontology.yaml"),
                ("memory-taxonomy", "memory/taxonomy.yaml"),
            ],
            "memory/policy-local.yaml",
        ),
        "code" => (
            vec![
                ("core", "core/ontology.yaml"),
                ("code", "code/ontology.yaml"),
                ("eng-taxonomy", "eng/taxonomy.yaml"),
            ],
            "code/policy-local.yaml",
        ),
        other => return Err(AllodError::Other(format!("unknown profile {other:?}"))),
    };

    let mut docs = Vec::new();
    for (name, rel) in &files {
        docs.push((name.to_string(), read(schema_dir.join(rel))?));
    }
    let policy = read(schema_dir.join(policy_path))?;

    Ok(ProfileSource {
        name: profile.to_string(),
        docs,
        policy,
    })
}

/// Genesis: install schema + policy, create the owner principal, self-admit the first changeset.
/// The Graph must be freshly created (Graph::create or Graph::with_store) before calling.
pub fn init(graph: &Graph, owner: &str, mut profile: ProfileSource) -> Result<InitResult, AllodError> {
    use allod_core::fold::State;
    use allod_core::sign::Keypair;

    let kp = Keypair::generate(owner);
    graph.save_key(&kp).map_err(AllodError::from)?;

    // Install schema docs
    for (name, doc) in &profile.docs {
        graph.install_schema(name, doc).map_err(AllodError::from)?;
    }

    // Bind owner into every policy role
    if let Some(roles) = profile.policy.get_mut("roles").and_then(Value::as_mapping_mut) {
        let bind = Value::Sequence(vec![Value::String(format!("principal:{owner}"))]);
        let names: Vec<Value> = roles.keys().cloned().collect();
        for name in names {
            roles.insert(name, bind.clone());
        }
    }
    graph.install_schema("policy", &profile.policy).map_err(AllodError::from)?;

    // Genesis changeset: create the owner principal node
    let owner_node = crate::ops::uuid4();
    let mut attrs = serde_yaml::Mapping::new();
    attrs.insert(Value::String("display_name".into()), Value::String(owner.into()));
    attrs.insert(Value::String("keys".into()), Value::Sequence(vec![crate::ops::key_record(&kp)]));
    attrs.insert(Value::String("status".into()), Value::String("active".into()));
    let node_op = crate::ops::create_node_op(&owner_node, "core/User@1", Value::Mapping(attrs), None);

    let intent = format!(
        "Genesis: root authority {owner}, core + {} schema, {}-local policy",
        profile.name, profile.name
    );
    let (cs, hash) = crate::ops::build_changeset(graph, &kp, &intent, vec![node_op])?;

    let reg = graph.registry().map_err(AllodError::from)?;
    let mut state = State::default();
    state.apply_changeset(&reg, &cs).map_err(AllodError::from)?;
    graph.append_changeset(&cs, &hash, None).map_err(AllodError::from)?;
    graph.write_meta(&hash, &[format!("principal:{owner}")]).map_err(AllodError::from)?;

    Ok(InitResult {
        graph_id: hash,
        owner: owner.to_string(),
    })
}

/// Register a new principal (agent, service, or user) in the graph.
/// Generates a fresh keypair for `name`, saves it, and admits or holds the changeset.
pub fn principal_add(graph: &Graph, name: &str, kind: &str, by: &str) -> Result<PrincipalAdded, AllodError> {
    use allod_core::sign::Keypair;
    use allod_core::get_str;
    use crate::ops;

    let type_ref = match kind {
        "agent"   => "core/Agent@1",
        "service" => "core/Service@1",
        "user"    => "core/User@1",
        other     => return Err(AllodError::Other(format!("unknown principal kind {other:?}"))),
    };

    let kp = Keypair::generate(name);
    graph.save_key(&kp).map_err(AllodError::from)?;

    let node_id = ops::uuid4();
    let mut attrs = serde_yaml::Mapping::new();
    attrs.insert(Value::String("display_name".into()), Value::String(name.into()));
    attrs.insert(Value::String("keys".into()), Value::Sequence(vec![ops::key_record(&kp)]));
    attrs.insert(Value::String("status".into()), Value::String("active".into()));

    if kind == "agent" {
        let state = graph.fold().map_err(AllodError::from)?;
        let (_, owner_obj) = state
            .find_principal(&format!("principal:{by}"))
            .ok_or_else(|| AllodError::UnknownPrincipal(format!("principal:{by}")))?;
        let owner_node = get_str(&owner_obj.content, "id").unwrap_or("").to_string();
        attrs.insert(Value::String("delegated_by".into()), Value::String(format!("node:{owner_node}")));
        attrs.insert(
            Value::String("scope".into()),
            serde_yaml::from_str::<Value>("{ region: workspace }").unwrap(),
        );
    }

    let node_op = ops::create_node_op(&node_id, type_ref, Value::Mapping(attrs), None);

    let owner_kp = graph.load_key(by).map_err(AllodError::from)?;
    let (cs, hash) = ops::build_changeset(
        graph,
        &owner_kp,
        &format!("Register {kind} {name}, by {by}"),
        vec![node_op],
    )?;
    let admission = ops::admit_or_hold(graph, by, &cs, &hash, vec![])?;

    Ok(PrincipalAdded { node_id, admission })
}

fn provenance_val(agent: &str) -> Value {
    let mut prov = serde_yaml::Mapping::new();
    prov.insert(Value::String("derived_by".into()), Value::String(format!("principal:{agent}")));
    prov.insert(Value::String("method".into()), Value::String("model-assisted".into()));
    prov.insert(Value::String("tool".into()), Value::String("allod-demo-agent@0.1".into()));
    Value::Mapping(prov)
}

/// Write a scratch note for `agent` with `content`. Admits immediately under scratch-is-free.
pub fn note(graph: &Graph, agent: &str, content: &str) -> Result<NoteResult, AllodError> {
    let kp = graph.load_key(agent).map_err(AllodError::from)?;
    let note_id = crate::ops::uuid4();

    let mut attrs = serde_yaml::Mapping::new();
    attrs.insert(Value::String("content".into()), Value::String(content.into()));

    let node_op = crate::ops::create_node_op(
        &note_id,
        "memory/Note@1",
        Value::Mapping(attrs),
        Some(provenance_val(agent)),
    );
    let cls_op = crate::ops::classification_op(
        &format!("node:{note_id}"),
        "workspace/scratch@1",
        &format!("principal:{agent}"),
        "model-assisted",
    );

    let (cs, hash) = crate::ops::build_changeset(graph, &kp, "Scratch note", vec![node_op, cls_op])?;
    let admission = crate::ops::admit_or_hold(graph, agent, &cs, &hash, vec![])?;

    Ok(NoteResult { note_id, admission })
}

/// Propose a new Preference node. Classified as `work@1`, optionally linked to a source note.
/// Builds a self-attesting envelope; the changeset is typically held for owner decision.
pub fn propose_preference(
    graph: &Graph,
    agent: &str,
    statement: &str,
    strength: &str,
    from_note: Option<&str>,
) -> Result<ProposalResult, AllodError> {
    use allod_core::policy;
    use crate::ops;

    let kp = graph.load_key(agent).map_err(AllodError::from)?;
    let pref_id = ops::uuid4();

    let mut attrs = serde_yaml::Mapping::new();
    attrs.insert(Value::String("statement".into()), Value::String(statement.into()));
    attrs.insert(Value::String("strength".into()), Value::String(strength.into()));

    let node_op = ops::create_node_op(
        &pref_id,
        "memory/Preference@1",
        Value::Mapping(attrs),
        Some(provenance_val(agent)),
    );
    let cls_op = ops::classification_op(
        &format!("node:{pref_id}"),
        "work@1",
        &format!("principal:{agent}"),
        "model-assisted",
    );

    let mut op_list = vec![node_op, cls_op];

    if let Some(note_id) = from_note {
        let edge_op = ops::create_edge_op(
            &ops::uuid4(),
            "memory/relates_to@1",
            &format!("node:{pref_id}"),
            &format!("node:{note_id}"),
            None,
        );
        op_list.push(edge_op);
    }

    let (cs, hash) = ops::build_changeset(
        graph,
        &kp,
        &format!("Propose preference: {statement}"),
        op_list,
    )?;

    // Self-attesting envelope (§5.2): proves who signed, not what code ran.
    let mut statement_map = serde_yaml::Mapping::new();
    statement_map.insert(Value::String("changeset_hash".into()), Value::String(hash.clone()));
    let mut envelope = serde_yaml::Mapping::new();
    envelope.insert(Value::String("kind".into()), Value::String("attestation-envelope".into()));
    envelope.insert(Value::String("statement".into()), Value::Mapping(statement_map));
    envelope.insert(Value::String("attester".into()), Value::String(format!("principal:{agent}")));
    envelope.insert(Value::String("evidence".into()), Value::String("none".into()));
    envelope.insert(Value::String("evidence_type".into()), Value::String("none".into()));
    let mut envelope = Value::Mapping(envelope);
    let payload = policy::envelope_payload(&envelope).map_err(AllodError::from)?;
    if let Some(map) = envelope.as_mapping_mut() {
        map.insert(Value::String("signature".into()), Value::String(kp.sign(&payload)));
    }

    let admission = ops::admit_or_hold(graph, agent, &cs, &hash, vec![envelope])?;

    Ok(ProposalResult { hash, admission })
}

/// Apply a decision (approve/reject) to a held proposal.
/// Rejected and StillUnmet paths still write the signed decision record to evidence (auditable).
pub fn decide(graph: &Graph, hash: &str, by: &str, verdict: &str) -> Result<DecisionOutcome, AllodError> {
    use allod_core::{get_str, policy};

    let kp = graph.load_key(by).map_err(AllodError::from)?;
    let cs = graph.read_proposal(hash).map_err(AllodError::from)?;
    let evidence = graph.read_proposal_evidence(hash).map_err(AllodError::from)?;

    let mut decisions: Vec<Value> = evidence
        .get("decisions")
        .and_then(Value::as_sequence)
        .cloned()
        .unwrap_or_default();
    let envelopes: Vec<Value> = evidence
        .get("envelopes")
        .and_then(Value::as_sequence)
        .cloned()
        .unwrap_or_default();

    let policy_doc = graph.policy().map_err(AllodError::from)?;

    let mut record = serde_yaml::Mapping::new();
    record.insert(Value::String("kind".into()), Value::String("decision-record".into()));
    record.insert(Value::String("subject".into()), Value::String(hash.into()));
    record.insert(
        Value::String("policy_context".into()),
        Value::String(policy::policy_context(&policy_doc).map_err(AllodError::from)?),
    );
    record.insert(Value::String("verdict".into()), Value::String(verdict.into()));
    record.insert(Value::String("timestamp".into()), Value::String(crate::ops::now_iso()));
    let mut record = Value::Mapping(record);

    let payload = policy::decision_payload(&record).map_err(AllodError::from)?;
    let mut decider = serde_yaml::Mapping::new();
    decider.insert(Value::String("principal".into()), Value::String(format!("principal:{by}")));
    decider.insert(Value::String("signature".into()), Value::String(kp.sign(&payload)));
    if let Some(map) = record.as_mapping_mut() {
        map.insert(Value::String("deciders".into()), Value::Sequence(vec![Value::Mapping(decider)]));
    }
    decisions.push(record);

    if verdict == "reject" {
        graph.write_proposal_evidence(hash, &crate::ops::evidence_doc(&decisions, &envelopes))
            .map_err(AllodError::from)?;
        return Ok(DecisionOutcome::Rejected);
    }

    let reg = graph.registry().map_err(AllodError::from)?;
    let state = graph.fold().map_err(AllodError::from)?;
    let author_ref = get_str(
        cs.get("author").ok_or_else(|| AllodError::Other("proposal has no author".into()))?,
        "principal",
    )
    .ok_or_else(|| AllodError::Other("author has no principal".into()))?
    .to_string();
    let author_kind = state
        .find_principal(&author_ref)
        .map(|(kind, _)| kind.to_string())
        .ok_or_else(|| AllodError::UnknownPrincipal(author_ref.clone()))?;
    let checklist = policy::evaluate(&reg, &policy_doc, &state, &cs, &author_kind)
        .map_err(AllodError::from)?;
    let roots = graph.roots().map_err(AllodError::from)?;
    let sat = policy::check_satisfied_with(
        &state,
        &policy_doc,
        &roots,
        &cs,
        &author_ref,
        &checklist,
        &decisions,
        &envelopes,
        &graph.trusted_measurements().map_err(AllodError::from)?,
    )
    .map_err(AllodError::from)?;

    if !sat.unmet.is_empty() {
        graph.write_proposal_evidence(hash, &crate::ops::evidence_doc(&decisions, &envelopes))
            .map_err(AllodError::from)?;
        return Ok(DecisionOutcome::StillUnmet { unmet: sat.unmet });
    }

    let mut state = state;
    state.apply_changeset(&reg, &cs).map_err(AllodError::from)?;
    graph.append_changeset(&cs, hash, Some(&crate::ops::evidence_doc(&decisions, &envelopes)))
        .map_err(AllodError::from)?;
    graph.remove_proposal(hash).map_err(AllodError::from)?;

    Ok(DecisionOutcome::Admitted { degraded: sat.degraded })
}

pub fn classify(_graph: &Graph, _node_id: &str, _term: &str, _by: &str, _basis: &str) -> Result<Admission, AllodError> {
    Err(AllodError::Other("not implemented".into()))
}

pub fn checkpoint(_graph: &Graph, _by: &str) -> Result<CheckpointResult, AllodError> {
    Err(AllodError::Other("not implemented".into()))
}
