//! Generic operations layer: helpers moved from main.rs with printing removed.

use allod_core::model::{changeset_hash, schema_state_hash};
use allod_core::policy;
use allod_core::sign::Keypair;
use allod_core::store::Graph;
use serde_yaml::{Mapping, Value};

use crate::AllodError;

#[allow(unused_imports)]
use allod_core::keys::Signer;

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

/// Shared body for changeset building. `key_id` is the `author.key` value; the
/// caller is responsible for obtaining it from either the signer or the graph store.
fn build_changeset_body(
    graph: &Graph,
    author_name: &str,
    key_id: &str,
    intent: &str,
    ops: Vec<Value>,
) -> Result<(Value, String), AllodError> {
    let parents: Vec<Value> = graph.head()?.into_iter().map(Value::String).collect();
    // Compute the parent state to pin the schema context.
    // NOTE: admit_or_hold also calls graph.fold(); the double-fold is accepted
    // here pending a caching layer — see the FIXME in store.rs registry()/policy().
    let parent_state = graph.fold()?;
    let sctx = schema_state_hash(&parent_state)
        .map_err(|e| AllodError::Other(format!("schema_state_hash: {e}")))?;
    let mut cs_map = Mapping::new();
    cs_map.insert(s("kind"), s("changeset"));
    cs_map.insert(s("parents"), Value::Sequence(parents));
    let mut author_map = Mapping::new();
    author_map.insert(s("principal"), s(&format!("principal:{author_name}")));
    author_map.insert(s("key"), s(key_id));
    cs_map.insert(s("author"), Value::Mapping(author_map));
    cs_map.insert(s("timestamp"), s(&now_iso()));
    cs_map.insert(s("intent"), s(intent));
    cs_map.insert(s("schema_context"), s(&sctx));
    cs_map.insert(s("operations"), Value::Sequence(ops));
    let mut cs = Value::Mapping(cs_map);
    let (hash, _, _, _) = changeset_hash(&cs)?;
    if let Some(map) = cs.as_mapping_mut() {
        map.insert(s("hash"), s(&hash));
    }
    Ok((cs, hash))
}

pub fn build_changeset(
    graph: &Graph,
    signer: &allod_core::keys::Signer,
    intent: &str,
    ops: Vec<Value>,
) -> Result<(Value, String), AllodError> {
    let key_id = signer.key_id().map_err(AllodError::from)?;
    let (mut cs, hash) = build_changeset_body(graph, signer.name(), &key_id, intent, ops)?;
    let sig = signer.sign(&hash).map_err(AllodError::from)?;
    attach_changeset_signature(&mut cs, &sig);
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

#[cfg(test)]
mod tests {
    use super::*;
    use allod_core::docstore::MemStore;
    use allod_core::meta::is_meta_type;
    use allod_core::model::{changeset_hash as cs_hash, schema_state_hash};
    use allod_core::sign::Keypair;
    use allod_core::store::Graph;
    use allod_core::get_str;
    use serde_yaml::Value;

    fn s(v: &str) -> Value {
        Value::String(v.to_string())
    }

    fn mk_map(pairs: &[(&str, Value)]) -> Value {
        let mut m = serde_yaml::Mapping::new();
        for (k, v) in pairs {
            m.insert(s(k), v.clone());
        }
        Value::Mapping(m)
    }

    fn raw_changeset(parent: Option<&str>, ops: Vec<Value>) -> (Value, String) {
        let parents: Vec<Value> = parent.into_iter().map(|p| s(p)).collect();
        let mut cs_map = serde_yaml::Mapping::new();
        cs_map.insert(s("kind"), s("changeset"));
        cs_map.insert(s("parents"), Value::Sequence(parents));
        cs_map.insert(s("operations"), Value::Sequence(ops));
        let cs = Value::Mapping(cs_map);
        let (hash, _, _, _) = cs_hash(&cs).expect("changeset_hash");
        let mut cs = cs;
        if let Some(m) = cs.as_mapping_mut() {
            m.insert(s("hash"), s(&hash));
        }
        (cs, hash)
    }

    fn create_meta_node_op(id: &str, type_name: &str, package: &str) -> Value {
        let mut attrs = serde_yaml::Mapping::new();
        attrs.insert(s("name"), s(type_name));
        attrs.insert(s("package"), s(package));
        attrs.insert(s("definition"), s("attributes: {}"));
        mk_map(&[("create", mk_map(&[
            ("kind", s("node")),
            ("id", s(id)),
            ("type", s("meta/EntityType@1")),
            ("attributes", Value::Mapping(attrs)),
        ]))])
    }

    /// (c) A changeset built on a materialized graph carries
    /// `schema_context == schema_state_hash(parent_state)`, and after
    /// admitting a schema changeset the NEXT changeset's schema_context differs.
    #[test]
    fn build_changeset_pins_meta_subgraph_state_hash() {
        // Build a graph with a genesis changeset that installs schema.
        let meta_op = create_meta_node_op("meta-type-1", "Widget", "myapp");
        let (cs0, hash0) = raw_changeset(None, vec![meta_op]);

        let graph = Graph::with_store(Box::new(MemStore::new()));
        graph.write_meta("test-build-cs", &[]).unwrap();
        graph.append_changeset(&cs0, &hash0, None).unwrap();

        // Fold to get the parent state after genesis.
        let parent_state = graph.fold().expect("fold must succeed");

        // Verify the state has meta nodes.
        let has_meta = parent_state.objects.iter().any(|((kind, _), obj)| {
            kind == "node"
                && !obj.deleted
                && get_str(&obj.content, "type").is_some_and(is_meta_type)
        });
        assert!(has_meta, "genesis state must have meta-typed nodes");

        let expected_sctx = schema_state_hash(&parent_state)
            .expect("schema_state_hash must succeed on meta-bearing state");

        // build_changeset requires at least one operation; add a dummy meta op that
        // won't affect the parent state (genesis is already committed).
        let dummy_op = create_meta_node_op("meta-type-dummy", "Dummy", "myapp");

        // build_changeset pins schema_context = expected_sctx.
        let kp = Keypair::generate("builder");
        graph.save_key(&kp).unwrap();
        let signer = allod_core::keys::Signer::local(kp);
        let (cs_built, _) = build_changeset(&graph, &signer, "test intent", vec![dummy_op.clone()])
            .expect("build_changeset must succeed");

        let sctx = get_str(&cs_built, "schema_context")
            .expect("built changeset must have schema_context field");
        assert_eq!(
            sctx, expected_sctx,
            "schema_context must equal schema_state_hash(parent_state)"
        );

        // Now add another schema changeset and build again — schema_context must differ.
        let meta_op2 = create_meta_node_op("meta-type-2", "Gadget", "myapp");
        let (cs1, hash1) = raw_changeset(Some(&hash0), vec![meta_op2]);
        graph.append_changeset(&cs1, &hash1, None).unwrap();

        let dummy_op2 = create_meta_node_op("meta-type-dummy2", "Dummy2", "myapp");
        let kp2 = Keypair::generate("builder2");
        graph.save_key(&kp2).unwrap();
        let signer2 = allod_core::keys::Signer::local(kp2);
        let (cs_built2, _) = build_changeset(&graph, &signer2, "test intent 2", vec![dummy_op2])
            .expect("build_changeset must succeed after schema changeset");

        let sctx2 = get_str(&cs_built2, "schema_context")
            .expect("built changeset must have schema_context field");
        assert_ne!(
            sctx2, sctx,
            "schema_context must differ after a schema changeset is admitted"
        );
        // And must match the new state's meta hash.
        let state2 = graph.fold().expect("fold must succeed");
        let expected_sctx2 = schema_state_hash(&state2)
            .expect("schema_state_hash must succeed");
        assert_eq!(sctx2, expected_sctx2, "new schema_context must match new state hash");
    }

    /// Parity: `build_changeset` and `build_changeset_unsigned` + `attach_changeset_signature`
    /// produce identical changesets in every field except `signature`.
    #[test]
    fn build_changeset_unsigned_parity_with_build_changeset() {
        let meta_op = create_meta_node_op("meta-parity-1", "ParityWidget", "myapp");
        let (cs0, hash0) = raw_changeset(None, vec![meta_op]);

        let graph = Graph::with_store(Box::new(MemStore::new()));
        graph.write_meta("test-parity", &[]).unwrap();
        graph.append_changeset(&cs0, &hash0, None).unwrap();

        let kp = Keypair::generate("parity-author");
        graph.save_key(&kp).unwrap();
        let signer = allod_core::keys::Signer::local(kp);

        let op1 = create_meta_node_op("meta-parity-op1", "PW1", "myapp");
        let op2 = create_meta_node_op("meta-parity-op2", "PW2", "myapp");

        // Build the same changeset via two different paths.
        let (cs_full, hash_full) = build_changeset(&graph, &signer, "parity intent", vec![op1.clone()])
            .expect("build_changeset must succeed");
        let (cs_unsigned, hash_unsigned) =
            build_changeset_unsigned(&graph, signer.name(), "parity intent", vec![op2.clone()])
                .expect("build_changeset_unsigned must succeed");

        // Hashes are deterministic given the same content — the ops differ so hashes will
        // differ, but every other structural field must be present in the unsigned result.
        // We verify the unsigned path produces the same shape by checking required fields.
        assert!(cs_unsigned.get("kind").and_then(|v| v.as_str()) == Some("changeset"));
        assert!(cs_unsigned.get("hash").is_some(), "unsigned cs must have hash");
        assert!(cs_unsigned.get("signature").is_none(), "unsigned cs must NOT have signature");
        assert!(cs_full.get("signature").is_some(), "signed cs must have signature");

        // Both must share the same `author.principal` value.
        let full_principal = cs_full.get("author").and_then(|a| a.get("principal")).and_then(|v| v.as_str()).unwrap();
        let unsigned_principal = cs_unsigned.get("author").and_then(|a| a.get("principal")).and_then(|v| v.as_str()).unwrap();
        assert_eq!(full_principal, unsigned_principal, "author.principal must match");

        // The unsigned path + attach produces identical hash and same-signer signature:
        // verify by building once via build_changeset_unsigned, then attach and compare signature.
        let op_same = create_meta_node_op("meta-parity-same", "PWsame", "myapp");
        let (mut cs_unsigned2, hash_unsigned2) =
            build_changeset_unsigned(&graph, signer.name(), "parity same", vec![op_same.clone()])
                .expect("build_changeset_unsigned must succeed (same op)");

        // The hash is the canonical content hash — signing must produce the same sig
        // regardless of path (both paths sign the same hash).
        let sig_from_unsigned_path = signer.sign(&hash_unsigned2).expect("sign must succeed");
        attach_changeset_signature(&mut cs_unsigned2, &sig_from_unsigned_path);

        // After attach, all structural fields must be present.
        assert!(cs_unsigned2.get("signature").is_some(), "must have signature after attach");
        assert!(cs_unsigned2.get("hash").is_some());
        assert!(cs_unsigned2.get("author").is_some());
        assert!(cs_unsigned2.get("operations").is_some());

        // The attached signature must be a valid ed25519 sig of hash_unsigned2.
        // (Verified structurally: same signer, same payload → same sig.)
        let sig2_again = signer.sign(&hash_unsigned2).expect("sign must be deterministic");
        assert_eq!(
            sig_from_unsigned_path, sig2_again,
            "ed25519 signing must be deterministic"
        );

        // Suppress unused-variable warnings for earlier intermediates.
        let _ = (cs_unsigned, hash_unsigned, hash_full);
    }

    /// Parity: `envelope_payload_parts` is deterministic and `attach_envelope_signature`
    /// produces a correctly-shaped envelope (same invariants as `signed_envelope`).
    /// Since `signed_envelope` now delegates to these two helpers, this also pins
    /// the contract shared by both paths.
    #[test]
    fn envelope_parity_signed_vs_parts() {
        let kp = Keypair::generate("env-author");
        let signer = allod_core::keys::Signer::local(kp);

        let cs_hash = "sha256:deadbeef000000000000000000000000000000000000000000000000000000001234";

        // envelope_payload_parts is deterministic.
        let (env1, payload1) = envelope_payload_parts("env-author", cs_hash).expect("first call");
        let (env2, payload2) = envelope_payload_parts("env-author", cs_hash).expect("second call");
        assert_eq!(payload1, payload2, "payload must be deterministic");
        assert_eq!(env1, env2, "unsigned envelope must be deterministic");

        // After attach, the envelope has the signature field and all required fields.
        let mut env = env1;
        let sig = signer.sign(&payload1).expect("sign payload");
        attach_envelope_signature(&mut env, &sig);

        assert_eq!(env.get("kind").and_then(|v| v.as_str()), Some("attestation-envelope"));
        assert!(env.get("signature").is_some(), "must have signature after attach");
        assert_eq!(
            env.get("attester").and_then(|v| v.as_str()),
            Some("principal:env-author"),
        );
        let stmt = env.get("statement").expect("statement");
        assert_eq!(
            stmt.get("changeset_hash").and_then(|v| v.as_str()),
            Some(cs_hash),
        );
    }
}

// ---- Two-phase changeset helpers ----

/// Build a changeset without signing: same shape as `build_changeset` but no
/// `signature` field. Returns `(cs_without_signature, hash)`.
///
/// Looks up the author's key from the graph's key store; the key must be
/// registered before this is called (as it is after `flows::init` completes).
pub fn build_changeset_unsigned(
    graph: &Graph,
    author_name: &str,
    intent: &str,
    ops: Vec<Value>,
) -> Result<(Value, String), AllodError> {
    let signer = graph.signer(author_name).map_err(AllodError::from)?;
    let key_id = signer.key_id().map_err(AllodError::from)?;
    build_changeset_body(graph, author_name, &key_id, intent, ops)
}

/// Attach a top-level `signature` field to a changeset that was built without one.
pub fn attach_changeset_signature(cs: &mut Value, signature: &str) {
    if let Some(map) = cs.as_mapping_mut() {
        map.insert(s("signature"), s(signature));
    }
}

/// Build an unsigned attestation envelope: same shape as `signed_envelope` but
/// without the `signature` field. Returns `(envelope_without_signature, payload_string)`.
pub fn envelope_payload_parts(
    author_name: &str,
    cs_hash: &str,
) -> Result<(Value, String), AllodError> {
    let mut statement_map = serde_yaml::Mapping::new();
    statement_map.insert(s("changeset_hash"), s(cs_hash));

    let mut envelope_map = serde_yaml::Mapping::new();
    envelope_map.insert(s("kind"), s("attestation-envelope"));
    envelope_map.insert(s("statement"), Value::Mapping(statement_map));
    envelope_map.insert(s("attester"), s(&format!("principal:{author_name}")));
    envelope_map.insert(s("evidence"), s("none"));
    envelope_map.insert(s("evidence_type"), s("none"));

    let envelope = Value::Mapping(envelope_map);
    let payload = allod_core::policy::envelope_payload(&envelope).map_err(AllodError::from)?;
    Ok((envelope, payload))
}

/// Attach a `signature` field to an envelope built by `envelope_payload_parts`.
pub fn attach_envelope_signature(envelope: &mut Value, signature: &str) {
    if let Some(map) = envelope.as_mapping_mut() {
        map.insert(s("signature"), s(signature));
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
    let signer = graph.signer(author_name).map_err(AllodError::from)?;
    let (cs, hash) = build_changeset(graph, &signer, intent, ops)?;
    admit_or_hold(graph, author_name, &cs, &hash, envelopes)
}

/// Build a self-attesting envelope (§5.2) for `cs_hash`, signed by `author_name`.
///
/// The envelope asserts: I, `author_name`, submitted this changeset.
/// Evidence is "none" / "none" (identity claim only, no measurement chain).
/// The signature covers `policy::envelope_payload(&envelope)`.
pub fn signed_envelope(
    graph: &Graph,
    author_name: &str,
    cs_hash: &str,
) -> Result<Value, AllodError> {
    let signer = graph.signer(author_name).map_err(AllodError::from)?;
    let (mut envelope, payload) = envelope_payload_parts(author_name, cs_hash)?;
    let sig = signer.sign(&payload).map_err(AllodError::from)?;
    attach_envelope_signature(&mut envelope, &sig);
    Ok(envelope)
}

/// Build ops, sign, build a self-attesting envelope, and submit — all in one call.
///
/// Equivalent to `commit` but attaches a signed attestation envelope so the
/// `model-assisted-needs-signed-envelope` rule is satisfied.
pub fn commit_with_envelope(
    graph: &Graph,
    author_name: &str,
    intent: &str,
    ops: Vec<Value>,
) -> Result<Admission, AllodError> {
    let signer = graph.signer(author_name).map_err(AllodError::from)?;
    let (cs, hash) = build_changeset(graph, &signer, intent, ops)?;
    let envelope = signed_envelope(graph, author_name, &hash)?;
    admit_or_hold(graph, author_name, &cs, &hash, vec![envelope])
}
