//! Generic operations layer: helpers moved from main.rs with printing removed.

use allod_core::model::{changeset_hash, schema_context};
use allod_core::policy;
use allod_core::sign::Keypair;
use allod_core::store::Graph;
use serde_yaml::{Mapping, Value};

use crate::AllodError;

fn s(v: &str) -> Value {
    Value::String(v.to_string())
}

// ---- The admission outcome ----

/// The admission outcome. `Held` is `Ok` — the system working as intended.
#[derive(Debug, serde::Serialize)]
pub enum Admission {
    Admitted { hash: String, matched_rules: Vec<String> },
    Held { hash: String, checklist: ChecklistView },
}

#[derive(Debug, serde::Serialize)]
pub struct ChecklistView {
    pub matched_rules: Vec<String>,
    pub reviewers: Vec<(String, u32)>,
    pub attestations: Vec<String>,
    pub root_required: bool,
}

// ---- Utility helpers ----

pub fn uuid4() -> String {
    let mut bytes: [u8; 16] = rand::random();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let h = allod_core::hash::hex_string(&bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

pub fn now_iso() -> String {
    #[cfg(target_arch = "wasm32")]
    let secs = (js_sys::Date::now() / 1000.0) as u64;
    #[cfg(not(target_arch = "wasm32"))]
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (days, rem) = (secs / 86400, secs % 86400);
    let z = days as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe as i64 + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

pub fn short(hash: &str) -> String {
    let h = hash.strip_prefix("sha256:").unwrap_or(hash);
    h.chars().take(12).collect()
}

// ---- Changeset construction ----

pub fn build_changeset(
    graph: &Graph,
    author: &Keypair,
    intent: &str,
    ops: Vec<Value>,
) -> Result<(Value, String), AllodError> {
    let parents: Vec<Value> = graph.head()?.into_iter().map(Value::String).collect();
    let sctx = schema_context(&graph.schema_docs()?)?;
    let mut cs = Mapping::new();
    cs.insert(s("kind"), s("changeset"));
    cs.insert(s("parents"), Value::Sequence(parents));
    let mut author_map = Mapping::new();
    author_map.insert(s("principal"), s(&format!("principal:{}", author.name)));
    author_map.insert(s("key"), s(&author.key_id()));
    cs.insert(s("author"), Value::Mapping(author_map));
    cs.insert(s("timestamp"), s(&now_iso()));
    cs.insert(s("intent"), s(intent));
    cs.insert(s("schema_context"), s(&sctx));
    cs.insert(s("operations"), Value::Sequence(ops));
    let mut cs = Value::Mapping(cs);
    let (hash, _, _, _) = changeset_hash(&cs)?;
    if let Some(map) = cs.as_mapping_mut() {
        map.insert(s("hash"), s(&hash));
        map.insert(s("signature"), s(&author.sign(&hash)));
    }
    Ok((cs, hash))
}

pub fn key_record(kp: &Keypair) -> Value {
    let mut record = Mapping::new();
    record.insert(s("key_id"), s(&kp.key_id()));
    record.insert(s("algorithm"), s("ed25519"));
    record.insert(s("public"), s(&kp.public_hex()));
    record.insert(s("status"), s("active"));
    Value::Mapping(record)
}

pub fn evidence_doc(decisions: &[Value], envelopes: &[Value]) -> Value {
    let mut map = Mapping::new();
    map.insert(s("decisions"), Value::Sequence(decisions.to_vec()));
    map.insert(s("envelopes"), Value::Sequence(envelopes.to_vec()));
    Value::Mapping(map)
}

// ---- Draft op builders ----

pub fn create_node_op(id: &str, type_ref: &str, attributes: Value, provenance: Option<Value>) -> Value {
    let mut node = Mapping::new();
    node.insert(s("kind"), s("node"));
    node.insert(s("id"), s(id));
    node.insert(s("type"), s(type_ref));
    node.insert(s("attributes"), attributes);
    if let Some(prov) = provenance {
        node.insert(s("provenance"), prov);
    }
    let mut op = Mapping::new();
    op.insert(s("create"), Value::Mapping(node));
    Value::Mapping(op)
}

pub fn create_edge_op(id: &str, type_ref: &str, from: &str, to: &str, attributes: Option<Value>) -> Value {
    let mut edge = Mapping::new();
    edge.insert(s("kind"), s("edge"));
    edge.insert(s("id"), s(id));
    edge.insert(s("type"), s(type_ref));
    edge.insert(s("from"), s(from));
    edge.insert(s("to"), s(to));
    if let Some(attrs) = attributes {
        edge.insert(s("attributes"), attrs);
    }
    let mut op = Mapping::new();
    op.insert(s("create"), Value::Mapping(edge));
    Value::Mapping(op)
}

pub fn classification_op(subject: &str, term: &str, asserted_by: &str, basis: &str) -> Value {
    let mut cls = Mapping::new();
    cls.insert(s("kind"), s("classification"));
    cls.insert(s("id"), s(&uuid4()));
    cls.insert(s("subject"), s(subject));
    cls.insert(s("term"), s(term));
    cls.insert(s("asserted_by"), s(asserted_by));
    cls.insert(s("basis"), s(basis));
    let mut op = Mapping::new();
    op.insert(s("create"), Value::Mapping(cls));
    Value::Mapping(op)
}

pub fn update_node_op(id: &str, prior_rev: &str, attributes: Value) -> Value {
    let mut node = Mapping::new();
    node.insert(s("kind"), s("node"));
    node.insert(s("id"), s(id));
    node.insert(s("prior_rev"), s(prior_rev));
    node.insert(s("attributes"), attributes);
    let mut op = Mapping::new();
    op.insert(s("update"), Value::Mapping(node));
    Value::Mapping(op)
}

pub fn delete_op(kind: &str, id: &str) -> Value {
    let mut obj = Mapping::new();
    obj.insert(s("kind"), s(kind));
    obj.insert(s("id"), s(id));
    let mut op = Mapping::new();
    op.insert(s("delete"), Value::Mapping(obj));
    Value::Mapping(op)
}

// ---- Admission ----

pub fn admit_or_hold(
    graph: &Graph,
    author_name: &str,
    cs: &Value,
    hash: &str,
    envelopes: Vec<Value>,
) -> Result<Admission, AllodError> {
    let reg = graph.registry()?;
    let policy = graph.policy()?;
    let state = graph.fold()?;
    let author_ref = format!("principal:{author_name}");
    let author_kind = state
        .find_principal(&author_ref)
        .map(|(kind, _)| kind.to_string())
        .ok_or_else(|| AllodError::UnknownPrincipal(author_ref.clone()))?;
    let checklist = policy::evaluate(&reg, &policy, &state, cs, &author_kind)?;
    let roots = graph.roots()?;
    let sat = policy::check_satisfied_with(
        &state,
        &policy,
        &roots,
        cs,
        &author_ref,
        &checklist,
        &[],
        &envelopes,
        &graph.trusted_measurements()?,
    )?;
    if sat.unmet.is_empty() {
        let mut state = state;
        state.apply_changeset(&reg, cs)?;
        let evidence = evidence_doc(&[], &envelopes);
        graph.append_changeset(cs, hash, Some(&evidence))?;
        Ok(Admission::Admitted {
            hash: hash.to_string(),
            matched_rules: checklist.matched_rules.into_iter().collect(),
        })
    } else {
        graph.write_proposal(cs, hash)?;
        graph.write_proposal_evidence(hash, &evidence_doc(&[], &envelopes))?;
        Ok(Admission::Held {
            hash: hash.to_string(),
            checklist: ChecklistView {
                matched_rules: checklist.matched_rules.into_iter().collect(),
                reviewers: checklist
                    .reviewers
                    .iter()
                    .map(|(r, q)| (r.clone(), *q as u32))
                    .collect(),
                attestations: checklist.attestations.clone(),
                root_required: checklist.root_required,
            },
        })
    }
}

/// Build ops, sign, and submit in one call.
pub fn commit(
    graph: &Graph,
    author_name: &str,
    intent: &str,
    ops: Vec<Value>,
    envelopes: Vec<Value>,
) -> Result<Admission, AllodError> {
    let kp = graph.load_key(author_name)?;
    let (cs, hash) = build_changeset(graph, &kp, intent, ops)?;
    admit_or_hold(graph, author_name, &cs, &hash, envelopes)
}
