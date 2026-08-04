//! The allod CLI: a governed knowledge graph on one machine, at
//! L2-enforced with the plain-keypair profile (§6.4.1).
//!
//! Commands:
//!   init <dir> --owner <name> [--schema <ontologies-dir>]
//!   agent-add <dir> <name> --by <owner>
//!   note <dir> --as <agent> <content…>
//!   propose-preference <dir> --as <agent> --statement <s>
//!       [--strength hard|soft] [--from-note <id>]
//!   proposals <dir>
//!   approve <dir> <proposal-hash> --as <principal>
//!   log <dir> | show <dir> | verify <dir>
//!   demo [dir] [--schema <ontologies-dir>]
//!
//! The demo runs the whole jarvis flow from the memory package: a
//! free scratch note, a governed preference promotion, and a full
//! verification pass.

mod fed;
mod repo;
mod md;

use allod_core::fold::State;
use allod_core::get_str;
use allod_core::model::{changeset_hash, schema_context};
use allod_core::policy::{self, Checklist};
use allod_core::sign::Keypair;
use allod_core::store::Graph;
use serde_yaml::{Mapping, Value};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn yaml(s: &str) -> Value {
    serde_yaml::from_str(s).expect("internal template must parse")
}

pub(crate) fn s(v: &str) -> Value {
    Value::String(v.to_string())
}

pub(crate) fn uuid4() -> String {
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

/// UTC now as RFC 3339, from the civil-from-days algorithm — no
/// clock dependencies beyond the system time.
pub(crate) fn now_iso() -> String {
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

pub(crate) fn short(hash: &str) -> String {
    let h = hash.strip_prefix("sha256:").unwrap_or(hash);
    h.chars().take(12).collect()
}

// ---------------- changeset construction ----------------

pub(crate) fn build_changeset(
    graph: &Graph,
    author: &Keypair,
    intent: &str,
    ops: Vec<Value>,
) -> Result<(Value, String), String> {
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

fn key_record(kp: &Keypair) -> Value {
    let mut record = Mapping::new();
    record.insert(s("key_id"), s(&kp.key_id()));
    record.insert(s("algorithm"), s("ed25519"));
    record.insert(s("public"), s(&kp.public_hex()));
    record.insert(s("status"), s("active"));
    Value::Mapping(record)
}

fn evidence_doc(decisions: &[Value], envelopes: &[Value]) -> Value {
    let mut map = Mapping::new();
    map.insert(s("decisions"), Value::Sequence(decisions.to_vec()));
    map.insert(s("envelopes"), Value::Sequence(envelopes.to_vec()));
    Value::Mapping(map)
}

fn print_checklist(checklist: &Checklist) {
    println!(
        "      matched rules: {}",
        checklist
            .matched_rules
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    );
    for (role, quorum) in &checklist.reviewers {
        println!("      requires: reviewers role {role} (quorum {quorum})");
    }
    for class in &checklist.attestations {
        println!("      requires: attestation from class {class}");
    }
    if checklist.root_required {
        println!("      requires: root authority (default posture)");
    }
}

/// Evaluate, then admit or hold. Returns true when admitted.
pub(crate) fn admit_or_hold(
    graph: &Graph,
    author_name: &str,
    cs: &Value,
    hash: &str,
    envelopes: Vec<Value>,
    quiet: bool,
) -> Result<bool, String> {
    let reg = graph.registry()?;
    let policy = graph.policy()?;
    let state = graph.fold()?;
    let author_ref = format!("principal:{author_name}");
    let author_kind = state
        .find_principal(&author_ref)
        .map(|(kind, _)| kind.to_string())
        .ok_or_else(|| format!("unknown principal {author_ref}"))?;
    let checklist = policy::evaluate(&reg, &policy, &state, cs, &author_kind)?;
    let roots = graph.roots()?;
    let sat = policy::check_satisfied_with(
        &state, &policy, &roots, cs, &author_ref, &checklist, &[], &envelopes,
        &graph.trusted_measurements()?,
    )?;
    if sat.unmet.is_empty() {
        let mut state = state;
        state.apply_changeset(&reg, cs)?;
        let evidence = evidence_doc(&[], &envelopes);
        graph.append_changeset(cs, hash, Some(&evidence))?;
        if !quiet {
            let basis = if checklist.matched_rules.is_empty() {
                "root authority, default posture".to_string()
            } else {
                format!(
                    "rules: {}",
                    checklist.matched_rules.iter().cloned().collect::<Vec<_>>().join(", ")
                )
            };
            println!("  ✓ admitted {} ({basis})", short(hash));
        }
        Ok(true)
    } else {
        graph.write_proposal(cs, hash)?;
        graph.write_proposal_evidence(hash, &evidence_doc(&[], &envelopes))?;
        if !quiet {
            println!("  ⧗ held as proposal {}", short(hash));
            print_checklist(&checklist);
        }
        Ok(false)
    }
}

// ---------------- commands ----------------

fn cmd_init(dir: &Path, owner: &str, schema_dir: &Path) -> Result<(), String> {
    cmd_init_profile(dir, owner, schema_dir, "memory")
}

fn cmd_init_profile(
    dir: &Path,
    owner: &str,
    schema_dir: &Path,
    profile: &str,
) -> Result<(), String> {
    let graph = Graph::create(dir)?;
    let kp = Keypair::generate(owner);
    graph.save_key(&kp)?;

    // Install the schema projection and the profile's local policy
    // with the owner bound into every role.
    let read = |p: PathBuf| -> Result<Value, String> {
        let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        serde_yaml::from_str(&text).map_err(|e| e.to_string())
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
        other => return Err(format!("unknown profile {other:?}")),
    };
    for (name, rel) in files {
        graph.install_schema(name, &read(schema_dir.join(rel))?)?;
    }
    let mut policy = read(schema_dir.join(policy_path))?;
    if let Some(roles) = policy.get_mut("roles").and_then(Value::as_mapping_mut) {
        let bind = Value::Sequence(vec![s(&format!("principal:{owner}"))]);
        let names: Vec<Value> = roles.keys().cloned().collect();
        for name in names {
            roles.insert(name, bind.clone());
        }
    }
    graph.install_schema("policy", &policy)?;

    // Genesis (§4.6): self-admitted by root signature, creating the
    // owner principal whose key signed it.
    let owner_node = uuid4();
    let mut attrs = Mapping::new();
    attrs.insert(s("display_name"), s(owner));
    attrs.insert(s("keys"), Value::Sequence(vec![key_record(&kp)]));
    attrs.insert(s("status"), s("active"));
    let mut node = Mapping::new();
    node.insert(s("kind"), s("node"));
    node.insert(s("id"), s(&owner_node));
    node.insert(s("type"), s("core/User@1"));
    node.insert(s("attributes"), Value::Mapping(attrs));
    let mut op = Mapping::new();
    op.insert(s("create"), Value::Mapping(node));

    let (cs, hash) = build_changeset(
        &graph,
        &kp,
        &format!("Genesis: root authority {owner}, core + memory schema, memory-local policy"),
        vec![Value::Mapping(op)],
    )?;
    let reg = graph.registry()?;
    let mut state = State::default();
    state.apply_changeset(&reg, &cs)?;
    graph.append_changeset(&cs, &hash, None)?;
    graph.write_meta(&hash, &[format!("principal:{owner}")])?;
    println!("  ✓ graph {} (owner {owner})", short(&hash));
    Ok(())
}

fn cmd_principal_add(dir: &Path, name: &str, kind: &str, by: &str) -> Result<(), String> {
    let graph = Graph::open(dir)?;
    let owner_kp = graph.load_key(by)?;
    let kp = Keypair::generate(name);
    graph.save_key(&kp)?;
    let type_ref = match kind {
        "agent" => "core/Agent@1",
        "service" => "core/Service@1",
        "user" => "core/User@1",
        other => return Err(format!("unknown principal kind {other:?}")),
    };
    let mut attrs = Mapping::new();
    attrs.insert(s("display_name"), s(name));
    attrs.insert(s("keys"), Value::Sequence(vec![key_record(&kp)]));
    attrs.insert(s("status"), s("active"));
    if kind == "agent" {
        let state = graph.fold()?;
        let (_, owner_obj) = state
            .find_principal(&format!("principal:{by}"))
            .ok_or_else(|| format!("unknown principal {by}"))?;
        let owner_node = get_str(&owner_obj.content, "id").unwrap_or("").to_string();
        attrs.insert(s("delegated_by"), s(&format!("node:{owner_node}")));
        attrs.insert(s("scope"), yaml("{ region: workspace }"));
    }
    let mut node = Mapping::new();
    node.insert(s("kind"), s("node"));
    node.insert(s("id"), s(&uuid4()));
    node.insert(s("type"), s(type_ref));
    node.insert(s("attributes"), Value::Mapping(attrs));
    let mut op = Mapping::new();
    op.insert(s("create"), Value::Mapping(node));

    let (cs, hash) = build_changeset(
        &graph,
        &owner_kp,
        &format!("Register {kind} {name}, by {by}"),
        vec![Value::Mapping(op)],
    )?;
    admit_or_hold(&graph, by, &cs, &hash, vec![], false)?;
    Ok(())
}

fn cmd_agent_add(dir: &Path, name: &str, by: &str) -> Result<(), String> {
    cmd_principal_add(dir, name, "agent", by)
}

fn provenance(agent: &str, tool: &str) -> Value {
    let mut prov = Mapping::new();
    prov.insert(s("derived_by"), s(&format!("principal:{agent}")));
    prov.insert(s("method"), s("model-assisted"));
    prov.insert(s("tool"), s(tool));
    Value::Mapping(prov)
}

fn classification_op(subject: &str, term: &str, agent: &str) -> Value {
    let mut cls = Mapping::new();
    cls.insert(s("kind"), s("classification"));
    cls.insert(s("id"), s(&uuid4()));
    cls.insert(s("subject"), s(subject));
    cls.insert(s("term"), s(term));
    cls.insert(s("asserted_by"), s(&format!("principal:{agent}")));
    cls.insert(s("basis"), s("model-assisted"));
    let mut op = Mapping::new();
    op.insert(s("create"), Value::Mapping(cls));
    Value::Mapping(op)
}

fn cmd_note(dir: &Path, agent: &str, content: &str) -> Result<(String, bool), String> {
    let graph = Graph::open(dir)?;
    let kp = graph.load_key(agent)?;
    let note_id = uuid4();
    let mut attrs = Mapping::new();
    attrs.insert(s("content"), s(content));
    let mut node = Mapping::new();
    node.insert(s("kind"), s("node"));
    node.insert(s("id"), s(&note_id));
    node.insert(s("type"), s("memory/Note@1"));
    node.insert(s("attributes"), Value::Mapping(attrs));
    node.insert(s("provenance"), provenance(agent, "allod-demo-agent@0.1"));
    let mut op = Mapping::new();
    op.insert(s("create"), Value::Mapping(node));

    let ops = vec![
        Value::Mapping(op),
        classification_op(&format!("node:{note_id}"), "workspace/scratch@1", agent),
    ];
    let (cs, hash) = build_changeset(&graph, &kp, "Scratch note", ops)?;
    let admitted = admit_or_hold(&graph, agent, &cs, &hash, vec![], false)?;
    Ok((note_id, admitted))
}

fn cmd_propose_preference(
    dir: &Path,
    agent: &str,
    statement: &str,
    strength: &str,
    from_note: Option<&str>,
) -> Result<String, String> {
    let graph = Graph::open(dir)?;
    let kp = graph.load_key(agent)?;
    let pref_id = uuid4();
    let mut attrs = Mapping::new();
    attrs.insert(s("statement"), s(statement));
    attrs.insert(s("strength"), s(strength));
    let mut node = Mapping::new();
    node.insert(s("kind"), s("node"));
    node.insert(s("id"), s(&pref_id));
    node.insert(s("type"), s("memory/Preference@1"));
    node.insert(s("attributes"), Value::Mapping(attrs));
    node.insert(s("provenance"), provenance(agent, "allod-demo-agent@0.1"));
    let mut op = Mapping::new();
    op.insert(s("create"), Value::Mapping(node));
    let mut ops = vec![
        Value::Mapping(op),
        classification_op(&format!("node:{pref_id}"), "work@1", agent),
    ];
    if let Some(note_id) = from_note {
        let mut edge = Mapping::new();
        edge.insert(s("kind"), s("edge"));
        edge.insert(s("id"), s(&uuid4()));
        edge.insert(s("type"), s("memory/relates_to@1"));
        edge.insert(s("from"), s(&format!("node:{pref_id}")));
        edge.insert(s("to"), s(&format!("node:{note_id}")));
        let mut op = Mapping::new();
        op.insert(s("create"), Value::Mapping(edge));
        ops.push(Value::Mapping(op));
    }
    let (cs, hash) = build_changeset(
        &graph,
        &kp,
        &format!("Propose preference: {statement}"),
        ops,
    )?;

    // The signed envelope (§5.2, evidence: none): the agent attests
    // its own pipeline. It proves who signed, not what code ran.
    let mut statement_map = Mapping::new();
    statement_map.insert(s("changeset_hash"), s(&hash));
    let mut envelope = Mapping::new();
    envelope.insert(s("kind"), s("attestation-envelope"));
    envelope.insert(s("statement"), Value::Mapping(statement_map));
    envelope.insert(s("attester"), s(&format!("principal:{agent}")));
    envelope.insert(s("evidence"), s("none"));
    envelope.insert(s("evidence_type"), s("none"));
    let mut envelope = Value::Mapping(envelope);
    let payload = policy::envelope_payload(&envelope)?;
    if let Some(map) = envelope.as_mapping_mut() {
        map.insert(s("signature"), s(&kp.sign(&payload)));
    }

    admit_or_hold(&graph, agent, &cs, &hash, vec![envelope], false)?;
    Ok(hash)
}

fn cmd_approve(dir: &Path, hash: &str, by: &str) -> Result<(), String> {
    cmd_decide(dir, hash, by, "approve")
}

fn cmd_reject(dir: &Path, hash: &str, by: &str) -> Result<(), String> {
    cmd_decide(dir, hash, by, "reject")
}

fn cmd_decide(dir: &Path, hash: &str, by: &str, verdict: &str) -> Result<(), String> {
    let graph = Graph::open(dir)?;
    let kp = graph.load_key(by)?;
    let cs = graph.read_proposal(hash)?;
    let evidence = graph.read_proposal_evidence(hash)?;
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

    let policy_doc = graph.policy()?;
    let mut record = Mapping::new();
    record.insert(s("kind"), s("decision-record"));
    record.insert(s("subject"), s(hash));
    record.insert(s("policy_context"), s(&policy::policy_context(&policy_doc)?));
    record.insert(s("verdict"), s(verdict));
    record.insert(s("timestamp"), s(&now_iso()));
    let mut record = Value::Mapping(record);
    let payload = policy::decision_payload(&record)?;
    let mut decider = Mapping::new();
    decider.insert(s("principal"), s(&format!("principal:{by}")));
    decider.insert(s("signature"), s(&kp.sign(&payload)));
    if let Some(map) = record.as_mapping_mut() {
        map.insert(s("deciders"), Value::Sequence(vec![Value::Mapping(decider)]));
    }
    decisions.push(record);

    if verdict == "reject" {
        // The proposal and its rejection stay auditable (§4.3,
        // Appendix A step 5): both remain on disk, and the signed
        // record is the evidence.
        graph.write_proposal_evidence(hash, &evidence_doc(&decisions, &envelopes))?;
        println!("  ✗ rejected {} — proposal and decision record stay auditable", short(hash));
        return Ok(());
    }

    let reg = graph.registry()?;
    let state = graph.fold()?;
    let author_ref = get_str(cs.get("author").ok_or("proposal has no author")?, "principal")
        .ok_or("author has no principal")?
        .to_string();
    let author_kind = state
        .find_principal(&author_ref)
        .map(|(kind, _)| kind.to_string())
        .ok_or_else(|| format!("unknown author {author_ref}"))?;
    let checklist = policy::evaluate(&reg, &policy_doc, &state, &cs, &author_kind)?;
    let roots = graph.roots()?;
    let sat = policy::check_satisfied_with(
        &state, &policy_doc, &roots, &cs, &author_ref, &checklist, &decisions, &envelopes,
        &graph.trusted_measurements()?,
    )?;
    if !sat.unmet.is_empty() {
        graph.write_proposal_evidence(hash, &evidence_doc(&decisions, &envelopes))?;
        println!("  ⧗ decision recorded; still unmet:");
        for item in &sat.unmet {
            println!("      - {item}");
        }
        return Ok(());
    }
    let mut state = state;
    state.apply_changeset(&reg, &cs)?;
    graph.append_changeset(&cs, hash, Some(&evidence_doc(&decisions, &envelopes)))?;
    graph.remove_proposal(hash)?;
    println!("  ✓ approved and admitted {}", short(hash));
    for note in &sat.degraded {
        println!("      degraded: {note}");
    }
    Ok(())
}

fn cmd_classify(
    dir: &Path,
    node_id: &str,
    term: &str,
    by: &str,
    basis: &str,
) -> Result<Option<String>, String> {
    let graph = Graph::open(dir)?;
    let kp = graph.load_key(by)?;
    let mut cls = Mapping::new();
    cls.insert(s("kind"), s("classification"));
    cls.insert(s("id"), s(&uuid4()));
    cls.insert(s("subject"), s(&format!("node:{node_id}")));
    cls.insert(s("term"), s(term));
    cls.insert(s("asserted_by"), s(&format!("principal:{by}")));
    cls.insert(s("basis"), s(basis));
    let mut op = Mapping::new();
    op.insert(s("create"), Value::Mapping(cls));
    let (cs, hash) = build_changeset(
        &graph,
        &kp,
        &format!("Classify node:{node_id} as {term}"),
        vec![Value::Mapping(op)],
    )?;
    let admitted = admit_or_hold(&graph, by, &cs, &hash, vec![], false)?;
    Ok(if admitted { None } else { Some(hash) })
}

fn cmd_checkpoint(dir: &Path, by: &str) -> Result<(), String> {
    let graph = Graph::open(dir)?;
    let kp = graph.load_key(by)?;
    let head = graph.head()?.ok_or("empty graph")?;
    let state = graph.fold()?;
    let mut cp = Mapping::new();
    cp.insert(s("kind"), s("checkpoint"));
    cp.insert(s("revision"), s(&head));
    cp.insert(s("state_hash"), s(&state.state_hash()?));
    cp.insert(s("state"), Value::Sequence(state.entries()));
    cp.insert(s("timestamp"), s(&now_iso()));
    cp.insert(s("signer"), s(&format!("principal:{by}")));
    let mut cp = Value::Mapping(cp);
    let payload = allod_core::sha256_hex(
        "checkpoint",
        &allod_core::canonical_cbor(&{
            let mut pre = cp.clone();
            pre.as_mapping_mut().unwrap().remove("signature");
            pre
        })?,
    );
    if let Some(map) = cp.as_mapping_mut() {
        map.insert(s("signature"), s(&kp.sign(&payload)));
    }
    graph.write_checkpoint(&head, &cp)?;
    println!("  ✓ checkpoint at {} (state {})", short(&head), short(
        get_str(&cp, "state_hash").unwrap_or("?")));
    Ok(())
}

fn checkpoint_payload(cp: &Value) -> Result<String, String> {
    let mut pre = cp.clone();
    if let Some(map) = pre.as_mapping_mut() {
        map.remove("signature");
    }
    Ok(allod_core::sha256_hex(
        "checkpoint",
        &allod_core::canonical_cbor(&pre)?,
    ))
}

fn cmd_trust(dir: &Path, measurement: &str) -> Result<(), String> {
    let graph = Graph::open(dir)?;
    graph.trust_measurement(measurement)?;
    println!("  ✓ trusting simulated measurement {}", short(measurement));
    Ok(())
}

/// Emit and verify one attestation envelope for an admitted
/// changeset (Appendix A step 8). Evidence is a simulated
/// measurement; the verification code path is the real one.
fn cmd_envelope(dir: &Path, cs_hash: &str, by: &str, tool: &str) -> Result<(), String> {
    let graph = Graph::open(dir)?;
    let kp = graph.load_key(by)?;
    let measurement = allod_core::hash::plain_sha256(tool.as_bytes());
    let mut statement = Mapping::new();
    statement.insert(s("changeset_hash"), s(cs_hash));
    let mut evidence = Mapping::new();
    evidence.insert(s("measurement"), s(&measurement));
    evidence.insert(s("claimed_identity"), s(tool));
    let mut envelope = Mapping::new();
    envelope.insert(s("kind"), s("attestation-envelope"));
    envelope.insert(s("statement"), Value::Mapping(statement));
    envelope.insert(s("attester"), s(&format!("principal:{by}")));
    envelope.insert(s("evidence"), Value::Mapping(evidence));
    envelope.insert(s("evidence_type"), s("simulated"));
    let mut envelope = Value::Mapping(envelope);
    let payload = policy::envelope_payload(&envelope)?;
    if let Some(map) = envelope.as_mapping_mut() {
        map.insert(s("signature"), s(&kp.sign(&payload)));
    }
    // Verify: signature against the attester's registered key, then
    // the evidence chain against the trusted measurements.
    let state = graph.fold()?;
    let attester_ref = format!("principal:{by}");
    let public = state
        .find_principal(&attester_ref)
        .and_then(|(_, obj)| {
            obj.content
                .get("attributes")?
                .get("keys")?
                .as_sequence()?
                .iter()
                .find_map(|r| get_str(r, "public").map(String::from))
        })
        .ok_or("attester has no registered key")?;
    allod_core::sign::verify(&public, &payload, get_str(&envelope, "signature").unwrap())?;
    match policy::verify_evidence(&envelope, &graph.trusted_measurements()?) {
        policy::EvidenceResult::Verified(note) => {
            println!("  ✓ envelope verified: {note}");
        }
        policy::EvidenceResult::Degraded(note) => println!("  ⚠ envelope degraded: {note}"),
        policy::EvidenceResult::Failed(reason) => {
            return Err(format!("envelope failed: {reason}"))
        }
    }
    // Attach it to the changeset's evidence for the audit trail.
    let mut evidence_file = graph
        .read_evidence(cs_hash)?
        .unwrap_or_else(|| evidence_doc(&[], &[]));
    if let Some(list) = evidence_file
        .as_mapping_mut()
        .and_then(|m| m.get_mut("envelopes"))
        .and_then(Value::as_sequence_mut)
    {
        list.push(envelope);
    }
    graph.write_evidence(cs_hash, &evidence_file)?;
    Ok(())
}

fn cmd_proposals(dir: &Path) -> Result<(), String> {
    let graph = Graph::open(dir)?;
    let proposals = graph.list_proposals()?;
    if proposals.is_empty() {
        println!("  no pending proposals");
    }
    for hash in proposals {
        let cs = graph.read_proposal(&hash)?;
        println!(
            "  {}  {}  by {}",
            short(&hash),
            get_str(&cs, "intent").unwrap_or(""),
            get_str(cs.get("author").unwrap_or(&Value::Null), "principal").unwrap_or("?")
        );
    }
    Ok(())
}

fn cmd_log(dir: &Path) -> Result<(), String> {
    let graph = Graph::open(dir)?;
    for cs in graph.chain()? {
        let hash = get_str(&cs, "hash").unwrap_or("?");
        let author = get_str(cs.get("author").unwrap_or(&Value::Null), "principal").unwrap_or("?");
        let ops = cs
            .get("operations")
            .and_then(Value::as_sequence)
            .map(|o| o.len())
            .unwrap_or(0);
        println!(
            "  {}  {}  [{ops} op{}]  {}",
            short(hash),
            author,
            if ops == 1 { "" } else { "s" },
            get_str(&cs, "intent").unwrap_or("")
        );
    }
    Ok(())
}

fn cmd_show(dir: &Path) -> Result<(), String> {
    let graph = Graph::open(dir)?;
    let state = graph.fold()?;
    println!("  state hash {}", short(&state.state_hash()?));
    for ((kind, _), obj) in &state.objects {
        if kind != "node" || obj.deleted {
            continue;
        }
        let tref = get_str(&obj.content, "type").unwrap_or("?");
        let attrs = obj.content.get("attributes");
        let label = attrs
            .and_then(|a| {
                get_str(a, "display_name")
                    .or_else(|| get_str(a, "statement"))
                    .or_else(|| get_str(a, "content"))
                    .or_else(|| get_str(a, "name"))
            })
            .unwrap_or("");
        let by = obj
            .content
            .get("provenance")
            .and_then(|p| get_str(p, "derived_by"))
            .map(|p| format!("  (by {p})"))
            .unwrap_or_default();
        println!("  {tref}  {label}{by}");
    }
    Ok(())
}

fn cmd_verify(dir: &Path) -> Result<(), String> {
    let graph = Graph::open(dir)?;
    let reg = graph.registry()?;
    let policy_doc = graph.policy()?;
    let roots = graph.roots()?;
    let chain = graph.chain()?;
    let mut state = State::default();
    let mut degraded: Vec<String> = Vec::new();
    for (i, cs) in chain.iter().enumerate() {
        let hash = get_str(cs, "hash").unwrap_or("?").to_string();
        let author = cs.get("author").cloned().unwrap_or(Value::Null);
        let author_ref = get_str(&author, "principal").unwrap_or("?").to_string();
        let key_id = get_str(&author, "key").unwrap_or("").to_string();
        let signature = get_str(cs, "signature").unwrap_or("").to_string();

        // Level 3 (§5.3): governance against the parent state, before
        // this changeset applies.
        let genesis = i == 0;
        let mut admitted_by = String::from("genesis (self-admitted, §4.6)");
        if !genesis {
            let author_kind = state
                .find_principal(&author_ref)
                .map(|(kind, _)| kind.to_string())
                .ok_or_else(|| format!("{hash}: unknown author {author_ref}"))?;
            let checklist = policy::evaluate(&reg, &policy_doc, &state, cs, &author_kind)?;
            let evidence = graph.read_evidence(&hash)?.unwrap_or(Value::Null);
            let decisions: Vec<Value> = evidence
                .get("decisions")
                .and_then(Value::as_sequence)
                .cloned()
                .unwrap_or_default();
            let envelopes: Vec<Value> = evidence
                .get("envelopes")
                .and_then(Value::as_sequence)
                .cloned()
                .unwrap_or_default();
            let sat = policy::check_satisfied_with(
                &state, &policy_doc, &roots, cs, &author_ref, &checklist, &decisions,
                &envelopes, &graph.trusted_measurements()?,
            )?;
            if !sat.unmet.is_empty() {
                return Err(format!(
                    "governance FAILS for {}: {}",
                    short(&hash),
                    sat.unmet.join("; ")
                ));
            }
            degraded.extend(sat.degraded);
            admitted_by = if checklist.is_trivial() {
                format!(
                    "rules {}",
                    checklist.matched_rules.iter().cloned().collect::<Vec<_>>().join(", ")
                )
            } else if !decisions.is_empty() {
                format!("{} decision record(s)", decisions.len())
            } else {
                "root authority".to_string()
            };
        }

        // Level 1: the fold recomputes and checks every hash.
        state.apply_changeset(&reg, cs).map_err(|e| format!("{hash}: {e}"))?;

        // Level 2 (§5.3): authorship. Genesis verifies against the key
        // it installs; later changesets against the parent-state key,
        // which for a linear log the post-apply lookup preserves
        // (rotation replay is the next milestone).
        let public = state
            .public_key_of(&author_ref, &key_id)
            .ok_or_else(|| format!("{hash}: no active key {key_id} for {author_ref}"))?;
        allod_core::sign::verify(&public, &hash, &signature)
            .map_err(|e| format!("{hash}: signature: {e}"))?;
        if genesis && !roots.contains(&author_ref) {
            return Err(format!("{hash}: genesis author {author_ref} is not root"));
        }

        println!(
            "  ✓ {}  integrity ✓  authorship ✓ ({author_ref})  governance: {admitted_by}",
            short(&hash)
        );
    }
    println!("  state hash {}", short(&state.state_hash()?));
    // Checkpoints (§3.2.5): replay MUST be able to verify each one.
    for cp in graph.checkpoints()? {
        let revision = get_str(&cp, "revision").unwrap_or("?").to_string();
        let claimed = get_str(&cp, "state_hash").unwrap_or("?").to_string();
        let signer = get_str(&cp, "signer").unwrap_or("?").to_string();
        let signature = get_str(&cp, "signature").unwrap_or("").to_string();
        if revision == graph.head()?.unwrap_or_default() && claimed != state.state_hash()? {
            return Err(format!("checkpoint at {} disagrees with replay", short(&revision)));
        }
        let payload = checkpoint_payload(&cp)?;
        let public = state
            .public_key_of(&signer, "")
            .or_else(|| {
                state.find_principal(&signer).and_then(|(_, obj)| {
                    obj.content
                        .get("attributes")?
                        .get("keys")?
                        .as_sequence()?
                        .iter()
                        .find_map(|r| get_str(r, "public").map(String::from))
                })
            })
            .ok_or_else(|| format!("checkpoint signer {signer} unknown"))?;
        allod_core::sign::verify(&public, &payload, &signature)
            .map_err(|e| format!("checkpoint signature: {e}"))?;
        println!("  ✓ checkpoint {} verified against replay ({signer})", short(&revision));
    }
    for note in &degraded {
        println!("  ⚠ degraded: {note}");
    }
    println!(
        "  VERIFIED: {} changesets, levels 1-3 (§5.3){}",
        chain.len(),
        if degraded.is_empty() { "" } else { ", with degradations noted" }
    );
    Ok(())
}

// ---------------- the demo ----------------

fn cmd_demo(dir: &Path, schema_dir: &Path) -> Result<(), String> {
    println!("allod agent-memory demo — the jarvis flow (ontologies/memory)");
    println!("═══════════════════════════════════════════════════════════════");
    println!("\n[1/7] Genesis: conner creates a graph, memory-local policy, restricted posture");
    cmd_init(dir, "conner", schema_dir)?;

    println!("\n[2/7] Register the agent: jarvis, delegated by conner (§6.1)");
    cmd_agent_add(dir, "jarvis", "conner")?;

    println!("\n[3/7] Jarvis writes a scratch note — thinks at full speed (rule scratch-is-free)");
    let (note_id, admitted) = cmd_note(
        dir,
        "jarvis",
        "Conner declined 4 of the last 5 meetings before 09:00, all with a reschedule note.",
    )?;
    if !admitted {
        return Err("scratch note should have admitted freely".into());
    }

    println!("\n[4/7] Jarvis proposes a Preference — held for the owner (§4.3)");
    let hash = cmd_propose_preference(
        dir,
        "jarvis",
        "No meetings before 09:00",
        "soft",
        Some(&note_id),
    )?;

    println!("\n[5/7] Conner approves — a signed, portable decision record admits it");
    cmd_approve(dir, &hash, "conner")?;

    println!("\n[6/7] Verify from the log and public keys alone (§5.3)");
    cmd_verify(dir)?;

    println!("\n[7/7] The memory:");
    cmd_show(dir)?;

    println!("\nThe sovereign-memory claim, demonstrated: the preference exists");
    println!("because conner signed, and any host — or no host — can verify that.");
    Ok(())
}

/// Federation demo (Appendix A step 9): two sovereign graphs, a
/// grant, a proofs-carrying bundle by file copy, a governed import,
/// and revocation.
fn cmd_demo_federation(dir_a: &Path, dir_b: &Path, schema: &Path) -> Result<(), String> {
    println!("allod federation demo — two sovereign graphs, one byte channel (a file)");
    println!("═══════════════════════════════════════════════════════════════════════");
    println!("\n[1/8] Graph A: conner + jarvis build the memory (the jarvis flow, quietly)");
    cmd_init(dir_a, "conner", schema)?;
    cmd_agent_add(dir_a, "jarvis", "conner")?;
    let (note_id, _) = cmd_note(
        dir_a,
        "jarvis",
        "Conner declined 4 of the last 5 meetings before 09:00.",
    )?;
    let hash = cmd_propose_preference(
        dir_a, "jarvis", "No meetings before 09:00", "soft", Some(&note_id),
    )?;
    cmd_approve(dir_a, &hash, "conner")?;

    println!("\n[2/8] Graph B: dana's own graph, its own root authority (§9.1)");
    cmd_init(dir_b, "dana", schema)?;

    println!("\n[3/8] B learns A's identity out of band: a peer record (§9.3)");
    let graph_a = Graph::open(dir_a)?;
    let a_id = get_str(&graph_a.meta()?, "graph_id").unwrap_or("").to_string();
    let a_key = graph_a.load_key("conner")?.public_hex();
    fed::peer_add(dir_b, "conner-memory", &a_id, &a_key, "dana")?;

    println!("\n[4/8] A issues a grant: region work, audience = B's graph ID (§9.4)");
    let graph_b = Graph::open(dir_b)?;
    let b_id = get_str(&graph_b.meta()?, "graph_id").unwrap_or("").to_string();
    let grant_id = fed::grant(dir_a, &b_id, "work", "conner")?;

    println!("\n[5/8] A produces a share bundle: objects with membership proofs (§9.5)");
    let bundle_path = std::env::temp_dir().join("allod-share-bundle.yaml");
    fed::bundle(dir_a, &grant_id, &bundle_path, "conner")?;

    println!("\n[6/8] B verifies the bundle with A's keys and imports the preference (§9.6)");
    let state_a = graph_a.fold()?;
    let pref_id = state_a
        .objects
        .iter()
        .find_map(|((kind, id), obj)| {
            (kind == "node"
                && get_str(&obj.content, "type").map(allod_core::bare)
                    == Some("memory/Preference"))
            .then(|| id.clone())
        })
        .ok_or("no preference in A")?;
    let held = fed::import(dir_b, &bundle_path, "dana", Some(&pref_id))?;
    if let Some(proposal) = held {
        println!("      B's own policy held the foreign preference — receiver-governed");
        println!("      exchange (§9.6, design principle 6). Dana decides:");
        cmd_approve(dir_b, &proposal, "dana")?;
    }

    println!("\n[7/8] B verifies its own graph — the import's lineage crosses intact");
    cmd_verify(dir_b)?;

    println!("\n[8/8] A revokes the grant; the next bundle request is refused (§9.4)");
    fed::revoke(dir_a, &grant_id, "conner")?;
    match fed::bundle(dir_a, &grant_id, &bundle_path, "conner") {
        Err(e) => println!("  ✓ refused: {e}"),
        Ok(()) => return Err("bundle should be refused after revocation".into()),
    }
    println!("\nBoth graphs stayed sovereign: B admitted under its own policy, and");
    println!("the claim in B still proves where it came from, by hash, forever.");
    Ok(())
}

/// Code-graph demo (Appendix A steps 2-4 and 8): commit-aligned
/// derivation, governed classification, the semantic diff with blast
/// radius, and one attested (simulated) envelope.
fn cmd_demo_code(dir: &Path, schema: &Path) -> Result<(), String> {
    println!("allod code-graph demo — a repository as governed knowledge (§8.3)");
    println!("═══════════════════════════════════════════════════════════════════");
    let repo_dir = dir.with_extension("repo");
    println!("\n[1/8] A sample repository: two commits on a spend path");
    let (first, second) = repo::make_sample_repo(&repo_dir)?;
    println!("  ✓ {} then {}", short(&first), short(&second));

    println!("\n[2/8] Genesis: code profile — core + code ontologies, eng taxonomy");
    cmd_init_profile(dir, "conner", schema, "code")?;
    cmd_principal_add(dir, "indexer", "service", "conner")?;
    cmd_principal_add(dir, "maria", "user", "conner")?;

    println!("\n[3/8] Import commit one — deterministic indexer writes admit (§8.1)");
    let (_h1, admitted) = repo::import_commit(dir, &repo_dir, &first, "indexer")?;
    if !admitted {
        return Err("first import should admit under the deterministic rule".into());
    }

    println!("\n[4/8] Maria proposes: authorize_spend is security/critical (Appendix A step 4)");
    let graph = Graph::open(dir)?;
    let state = graph.fold()?;
    let fn_id = state
        .objects
        .iter()
        .find_map(|((kind, id), obj)| {
            (kind == "node"
                && obj.content.get("attributes").and_then(|a| get_str(a, "name"))
                    == Some("authorize_spend"))
            .then(|| id.clone())
        })
        .ok_or("authorize_spend not derived")?;
    let held = cmd_classify(dir, &fn_id, "security/critical@1", "maria", "manual")?;
    let proposal = held.ok_or("classification should be held for the owner")?;
    println!("      the owner signs off:");
    cmd_approve(dir, &proposal, "conner")?;

    println!("\n[5/8] Import commit two — it touches classified code, so it is held (§8.3)");
    let (h2, admitted) = repo::import_commit(dir, &repo_dir, &second, "indexer")?;
    if admitted {
        return Err("second import touches security/critical and must be held".into());
    }

    println!("\n[6/8] The semantic diff, as a review artifact (§4.4): what changed, who calls it");
    repo::semantic_diff(dir, &h2, None)?;
    println!("      reviewed; the owner admits it:");
    cmd_approve(dir, &h2, "conner")?;

    println!("\n[7/8] One attested envelope, simulated evidence, real verification (step 8)");
    let measurement = allod_core::hash::plain_sha256(repo::SCAN_TOOL.as_bytes());
    cmd_trust(dir, &measurement)?;
    cmd_envelope(dir, &h2, "indexer", repo::SCAN_TOOL)?;

    println!("\n[8/8] Verify the whole graph from the log and keys");
    cmd_verify(dir)?;
    Ok(())
}

// ---------------- argument plumbing ----------------

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn positional(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut skip = false;
    for arg in args {
        if skip {
            skip = false;
            continue;
        }
        if arg.starts_with("--") {
            skip = true;
            continue;
        }
        out.push(arg.clone());
    }
    out
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().cloned().unwrap_or_default();
    let rest = &args[1.min(args.len())..];
    let pos = positional(rest);
    let dir = pos.first().map(PathBuf::from);
    let schema = flag(rest, "--schema").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from("ontologies")
    });

    let result = match command.as_str() {
        "init" => match (dir, flag(rest, "--owner")) {
            (Some(dir), Some(owner)) => cmd_init(&dir, &owner, &schema),
            _ => Err("usage: allod init <dir> --owner <name> [--schema <dir>]".into()),
        },
        "agent-add" => match (dir, pos.get(1), flag(rest, "--by")) {
            (Some(dir), Some(name), Some(by)) => cmd_agent_add(&dir, name, &by),
            _ => Err("usage: allod agent-add <dir> <name> --by <owner>".into()),
        },
        "principal-add" => match (dir, pos.get(1), flag(rest, "--kind"), flag(rest, "--by")) {
            (Some(dir), Some(name), Some(kind), Some(by)) => {
                cmd_principal_add(&dir, name, &kind, &by)
            }
            _ => Err("usage: allod principal-add <dir> <name> --kind user|service|agent --by <owner>".into()),
        },
        "classify" => match (dir, pos.get(1), pos.get(2), flag(rest, "--as")) {
            (Some(dir), Some(node), Some(term), Some(by)) => cmd_classify(
                &dir, node, term, &by,
                &flag(rest, "--basis").unwrap_or_else(|| "manual".into()),
            ).map(|_| ()),
            _ => Err("usage: allod classify <dir> <node-id> <term> --as <principal> [--basis b]".into()),
        },
        "reject" => match (dir, pos.get(1), flag(rest, "--as")) {
            (Some(dir), Some(hash), Some(by)) => cmd_reject(&dir, hash, &by),
            _ => Err("usage: allod reject <dir> <proposal-hash> --as <principal>".into()),
        },
        "checkpoint" => match (dir, flag(rest, "--as")) {
            (Some(dir), Some(by)) => cmd_checkpoint(&dir, &by),
            _ => Err("usage: allod checkpoint <dir> --as <principal>".into()),
        },
        "trust" => match (dir, pos.get(1)) {
            (Some(dir), Some(m)) => cmd_trust(&dir, m),
            _ => Err("usage: allod trust <dir> <measurement-hash>".into()),
        },
        "envelope" => match (dir, pos.get(1), flag(rest, "--as"), flag(rest, "--tool")) {
            (Some(dir), Some(hash), Some(by), Some(tool)) => {
                cmd_envelope(&dir, hash, &by, &tool)
            }
            _ => Err("usage: allod envelope <dir> <cs-hash> --as <principal> --tool <identity>".into()),
        },
        "note" => match (dir, flag(rest, "--as")) {
            (Some(dir), Some(agent)) => {
                let content = pos[1..].join(" ");
                cmd_note(&dir, &agent, &content).map(|_| ())
            }
            _ => Err("usage: allod note <dir> --as <agent> <content…>".into()),
        },
        "propose-preference" => match (dir, flag(rest, "--as"), flag(rest, "--statement")) {
            (Some(dir), Some(agent), Some(statement)) => cmd_propose_preference(
                &dir,
                &agent,
                &statement,
                &flag(rest, "--strength").unwrap_or_else(|| "soft".into()),
                flag(rest, "--from-note").as_deref(),
            )
            .map(|_| ()),
            _ => Err(
                "usage: allod propose-preference <dir> --as <agent> --statement <s> \
                 [--strength hard|soft] [--from-note <id>]"
                    .into(),
            ),
        },
        "approve" => match (dir, pos.get(1), flag(rest, "--as")) {
            (Some(dir), Some(hash), Some(by)) => cmd_approve(&dir, hash, &by),
            _ => Err("usage: allod approve <dir> <proposal-hash> --as <principal>".into()),
        },
        "peer-add" => match (dir, pos.get(1), flag(rest, "--graph-id"), flag(rest, "--key"), flag(rest, "--by")) {
            (Some(dir), Some(name), Some(gid), Some(key), Some(by)) => {
                fed::peer_add(&dir, name, &gid, &key, &by)
            }
            _ => Err("usage: allod peer-add <dir> <name> --graph-id <hash> --key <public-hex> --by <principal>".into()),
        },
        "grant" => match (dir, flag(rest, "--audience"), flag(rest, "--region"), flag(rest, "--as")) {
            (Some(dir), Some(aud), Some(region), Some(by)) => {
                fed::grant(&dir, &aud, &region, &by).map(|id| println!("  grant id {id}"))
            }
            _ => Err("usage: allod grant <dir> --audience <graph-id|public> --region <term> --as <principal>".into()),
        },
        "grant-revoke" => match (dir, pos.get(1), flag(rest, "--as")) {
            (Some(dir), Some(id), Some(by)) => fed::revoke(&dir, id, &by),
            _ => Err("usage: allod grant-revoke <dir> <grant-id> --as <principal>".into()),
        },
        "bundle" => match (dir, pos.get(1), pos.get(2), flag(rest, "--as")) {
            (Some(dir), Some(grant_id), Some(out), Some(by)) => {
                fed::bundle(&dir, grant_id, Path::new(out), &by)
            }
            _ => Err("usage: allod bundle <dir> <grant-id> <out-file> --as <principal>".into()),
        },
        "bundle-import" => match (dir, pos.get(1), flag(rest, "--as")) {
            (Some(dir), Some(bundle), Some(by)) => fed::import(
                &dir, Path::new(bundle), &by, flag(rest, "--import").as_deref(),
            ).map(|_| ()),
            _ => Err("usage: allod bundle-import <dir> <bundle-file> --as <principal> [--import <node-id>]".into()),
        },
        "init-code" => match (dir, flag(rest, "--owner")) {
            (Some(dir), Some(owner)) => cmd_init_profile(&dir, &owner, &schema, "code"),
            _ => Err("usage: allod init-code <dir> --owner <name> [--schema <dir>]".into()),
        },
        "repo-import" => match (dir, pos.get(1), pos.get(2), flag(rest, "--as")) {
            (Some(dir), Some(repo_path), Some(commit), Some(by)) => {
                repo::import_commit(&dir, Path::new(repo_path), commit, &by).map(|_| ())
            }
            _ => Err("usage: allod repo-import <dir> <repo> <commit> --as <service>".into()),
        },
        "semantic-diff" => match (dir, pos.get(1)) {
            (Some(dir), Some(hash)) => repo::semantic_diff(
                &dir, hash, flag(rest, "--out").as_deref().map(Path::new),
            ),
            _ => Err("usage: allod semantic-diff <dir> <cs-hash> [--out <file>]".into()),
        },
        "demo-code" => {
            let dir = dir.unwrap_or_else(|| PathBuf::from("allod-demo-code"));
            cmd_demo_code(&dir, &schema)
        }
        "demo-federation" => {
            let a = dir.unwrap_or_else(|| PathBuf::from("allod-demo-a"));
            let b = pos.get(1).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("allod-demo-b"));
            cmd_demo_federation(&a, &b, &schema)
        }
        "export-md" => match (dir, pos.get(1)) {
            (Some(dir), Some(out)) => md::export(&dir, Path::new(out)),
            _ => Err("usage: allod export-md <dir> <out-dir>".into()),
        },
        "import-md" => match (dir, pos.get(1), flag(rest, "--as")) {
            (Some(dir), Some(bundle), Some(by)) => md::import(&dir, Path::new(bundle), &by),
            _ => Err("usage: allod import-md <dir> <bundle-dir> --as <principal>".into()),
        },
        "proposals" => dir.ok_or("usage: allod proposals <dir>".to_string())
            .and_then(|d| cmd_proposals(&d)),
        "log" => dir.ok_or("usage: allod log <dir>".to_string()).and_then(|d| cmd_log(&d)),
        "show" => dir.ok_or("usage: allod show <dir>".to_string()).and_then(|d| cmd_show(&d)),
        "verify" => dir
            .ok_or("usage: allod verify <dir>".to_string())
            .and_then(|d| cmd_verify(&d)),
        "demo" => {
            let dir = dir.unwrap_or_else(|| PathBuf::from("allod-demo"));
            cmd_demo(&dir, &schema)
        }
        _ => Err(
            "usage: allod <init|agent-add|note|propose-preference|proposals|approve|log|show|verify|demo> …"
                .into(),
        ),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
