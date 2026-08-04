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

pub fn profile_from_dir(_profile: &str, _schema_dir: &Path) -> Result<ProfileSource, AllodError> {
    Err(AllodError::Other("not implemented".into()))
}

pub fn init(_graph: &Graph, _owner: &str, _profile: ProfileSource) -> Result<InitResult, AllodError> {
    Err(AllodError::Other("not implemented".into()))
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
