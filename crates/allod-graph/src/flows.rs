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

pub fn principal_add(_graph: &Graph, _name: &str, _kind: &str, _by: &str) -> Result<PrincipalAdded, AllodError> {
    Err(AllodError::Other("not implemented".into()))
}

pub fn note(_graph: &Graph, _agent: &str, _content: &str) -> Result<NoteResult, AllodError> {
    Err(AllodError::Other("not implemented".into()))
}

pub fn propose_preference(
    _graph: &Graph,
    _agent: &str,
    _statement: &str,
    _strength: &str,
    _from_note: Option<&str>,
) -> Result<ProposalResult, AllodError> {
    Err(AllodError::Other("not implemented".into()))
}

pub fn decide(_graph: &Graph, _hash: &str, _by: &str, _verdict: &str) -> Result<DecisionOutcome, AllodError> {
    Err(AllodError::Other("not implemented".into()))
}

pub fn classify(_graph: &Graph, _node_id: &str, _term: &str, _by: &str, _basis: &str) -> Result<Admission, AllodError> {
    Err(AllodError::Other("not implemented".into()))
}

pub fn checkpoint(_graph: &Graph, _by: &str) -> Result<CheckpointResult, AllodError> {
    Err(AllodError::Other("not implemented".into()))
}
