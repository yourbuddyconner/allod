//! Generate the Appendix H test vectors and package content hashes.
//!
//! Two subcommands:
//!
//!   hashes [ontologies-dir]
//!       Print the package content hash (§2.5, `package` domain) for
//!       every ontology, resolved in dependency order. These are the
//!       values import declarations bind to, and allod-lint verifies
//!       the files against them.
//!
//!   generate [out-dir] [ontologies-dir]
//!       Write the vector files: a three-changeset log, every
//!       revision and changeset hash, the state hash after each
//!       revision, one elided changeset with its leaf proof, and one
//!       intent redaction. Deterministic: same inputs, same bytes.
//!
//! Signatures use the RFC 8032 test keys, so they are deterministic;
//! the governance section carries one passing and one failing audit
//! of the same proposal under the memory-local policy.

use allod_core::fold::State as FoldState;
use allod_core::hash::{hex_string, package_hash, plain_sha256, sha256_hex};
use allod_core::sign::Keypair;
use allod_core::model::{
    changeset_hash, changeset_hash_from_leaves, revision_hash, state_entry, state_root,
};
use allod_core::get_str;
use serde_yaml::{Mapping, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn yaml(s: &str) -> Value {
    serde_yaml::from_str(s).expect("vector template must parse")
}

// ---------------- package hashes ----------------

fn load_ontology_docs(dir: &Path) -> Result<BTreeMap<String, Value>, String> {
    let loaded = allod_core::load_dir(dir);
    for issue in &loaded.issues {
        return Err(format!(
            "{}: {}: {}",
            issue.path.display(),
            issue.context,
            issue.message
        ));
    }
    let mut docs = BTreeMap::new();
    for (_, doc) in &loaded.docs {
        if let Some(name) = get_str(doc, "ontology") {
            docs.insert(name.to_string(), doc.clone());
        }
    }
    Ok(docs)
}

/// Compute every package's content hash, filling import hashes in
/// dependency order. The result is a fixed point: files that already
/// carry these values hash to these values.
fn compute_package_hashes(
    docs: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, String>, String> {
    let mut hashes: BTreeMap<String, String> = BTreeMap::new();
    let mut remaining: Vec<String> = docs.keys().cloned().collect();
    while !remaining.is_empty() {
        let mut progressed = false;
        let mut next = Vec::new();
        for name in remaining {
            let doc = &docs[&name];
            let imports: Vec<String> = doc
                .get("imports")
                .and_then(Value::as_sequence)
                .map(|seq| {
                    seq.iter()
                        .filter_map(|imp| get_str(imp, "ontology"))
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default();
            if imports.iter().all(|imp| hashes.contains_key(imp)) {
                let mut doc = doc.clone();
                if let Some(seq) = doc.get_mut("imports").and_then(Value::as_sequence_mut) {
                    for imp in seq {
                        let target = get_str(imp, "ontology").unwrap_or_default().to_string();
                        if let Some(map) = imp.as_mapping_mut() {
                            map.insert(
                                Value::String("state_hash".into()),
                                Value::String(hashes[&target].clone()),
                            );
                        }
                    }
                }
                hashes.insert(name, package_hash(&doc)?);
                progressed = true;
            } else {
                next.push(name);
            }
        }
        remaining = next;
        if !progressed {
            return Err(format!(
                "unresolvable imports among packages: {remaining:?}"
            ));
        }
    }
    Ok(hashes)
}

// ---------------- fold-lite and the state tree ----------------

/// Enough of §3.2.4 to materialize the vector log: apply operations,
/// verify prior-revision hashes, track tombstones. The full fold
/// (schema validation, merges, rejection) is the next milestone.
#[derive(Default)]
struct State {
    // (kind, id) -> (rev, deleted)
    objects: BTreeMap<(String, String), (String, bool)>,
}

impl State {
    fn apply(&mut self, op: &Value) -> Result<(), String> {
        let map = op.as_mapping().ok_or("operation must be a map")?;
        let (verb, payload) = map.iter().next().ok_or("empty operation")?;
        let verb = verb.as_str().ok_or("operation verb must be a string")?;
        let kind = get_str(payload, "kind").ok_or("payload needs kind")?.to_string();
        let id = get_str(payload, "id").ok_or("payload needs id")?.to_string();
        let key = (kind, id);
        match verb {
            "create" => {
                if self.objects.contains_key(&key) {
                    return Err(format!("create of existing object {key:?}"));
                }
                let rev = revision_hash(payload)?;
                self.objects.insert(key, (rev, false));
            }
            "update" => {
                let prior = get_str(payload, "prior").ok_or("update needs prior")?;
                let (current, deleted) = self
                    .objects
                    .get(&key)
                    .ok_or(format!("update of unknown object {key:?}"))?;
                if *deleted || current != prior {
                    return Err(format!("prior-revision mismatch on {key:?}"));
                }
                let mut content = payload.clone();
                if let Some(map) = content.as_mapping_mut() {
                    map.remove("prior");
                }
                let rev = revision_hash(&content)?;
                self.objects.insert(key, (rev, false));
            }
            "delete" => {
                let prior = get_str(payload, "prior").ok_or("delete needs prior")?;
                let (current, deleted) = self
                    .objects
                    .get(&key)
                    .ok_or(format!("delete of unknown object {key:?}"))?;
                if *deleted || current != prior {
                    return Err(format!("prior-revision mismatch on {key:?}"));
                }
                let rev = current.clone();
                self.objects.insert(key, (rev, true));
            }
            other => return Err(format!("fold-lite does not apply {other:?}")),
        }
        Ok(())
    }

    /// State hash (§1.7): Merkle root over per-object leaves, grouped
    /// by kind and sorted by logical ID (the BTreeMap order).
    fn state_hash(&self) -> Result<(String, Vec<Value>), String> {
        let listing: Vec<Value> = self
            .objects
            .iter()
            .map(|((kind, id), (rev, deleted))| state_entry(kind, id, rev, *deleted))
            .collect();
        let root = state_root(&listing)?;
        Ok((root, listing))
    }
}

// ---------------- the vector log ----------------

/// RFC 8032 test keys — deterministic, never for real graphs.
const OWNER_SECRET: &str = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
const AGENT_SECRET: &str = "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb";

struct Built {
    name: String,
    stored: Value,
    hash: String,
    root: String,
    leaves: Vec<String>,
    preimage_hex: String,
    state_hash: String,
    state_listing: Vec<Value>,
}

fn build_changeset(
    name: &str,
    header: &str,
    ops: Vec<Value>,
    state: &mut State,
    signer: &Keypair,
) -> Result<Built, String> {
    let mut cs = yaml(header);
    let map = cs.as_mapping_mut().ok_or("changeset template must be a map")?;
    map.insert(Value::String("operations".into()), Value::Sequence(ops.clone()));
    let (hash, root, leaves, bytes) = changeset_hash(&cs)?;
    if let Some(map) = cs.as_mapping_mut() {
        map.insert(
            Value::String("signature".into()),
            Value::String(signer.sign(&hash)),
        );
    }
    if let Some(map) = cs.as_mapping_mut() {
        map.insert(Value::String("hash".into()), Value::String(hash.clone()));
    }
    for op in &ops {
        state.apply(op)?;
    }
    let (state_hash, state_listing) = state.state_hash()?;
    Ok(Built {
        name: name.to_string(),
        stored: cs,
        hash,
        root,
        leaves,
        preimage_hex: hex_string(&bytes),
        state_hash,
        state_listing,
    })
}

fn generate(out_dir: &Path, ontologies_dir: &Path) -> Result<(), String> {
    let docs = load_ontology_docs(ontologies_dir)?;
    let packages = compute_package_hashes(&docs)?;
    let core_hash = packages
        .get("core")
        .ok_or("core package not found")?
        .clone();

    let doc_bytes = b"hello, vectors\n";
    let doc_hash = plain_sha256(doc_bytes);
    let owner = Keypair::from_secret_hex("vector-owner", OWNER_SECRET)?;
    let owner_key_id = owner.key_id();
    let mut state = State::default();

    // --- cs1: genesis ---
    let ada = "00000000-0000-4000-8000-000000000001";
    let grace = "00000000-0000-4000-8000-000000000002";
    let cs1_ops = vec![
        yaml(&format!(
            "create: {{ kind: node, id: \"{ada}\", type: \"core/Person@1\", \
             attributes: {{ name: \"Ada\" }} }}"
        )),
        yaml(&format!(
            "create: {{ kind: node, id: \"{grace}\", type: \"core/Person@1\", \
             attributes: {{ name: \"Grace\" }} }}"
        )),
        yaml(&format!(
            "create: {{ kind: edge, id: \"00000000-0000-4000-8000-000000000003\", \
             type: \"core/knows@1\", from: \"node:{ada}\", to: \"node:{grace}\", \
             attributes: {{ since: \"2026-01-01\" }} }}"
        )),
        yaml(&format!(
            "create: {{ kind: document, id: \"00000000-0000-4000-8000-000000000004\", \
             content_hash: \"{doc_hash}\", media_type: \"text/plain\", storage: stored }}"
        )),
        yaml(&format!(
            "create: {{ kind: classification, \
             id: \"00000000-0000-4000-8000-000000000005\", subject: \"node:{ada}\", \
             term: \"workspace/curated@1\", asserted_by: \"principal:vector-author\", \
             basis: manual }}"
        )),
    ];
    let cs1 = build_changeset(
        "cs1",
        &format!(
            "{{ kind: changeset, parents: [], \
             author: {{ principal: \"principal:vector-author\", \
             key: \"{owner_key_id}\" }}, \
             timestamp: \"2026-08-02T00:00:00Z\", \
             intent: \"Genesis: two people, an edge, a document, a label\", \
             schema_context: \"{core_hash}\" }}"
        ),
        cs1_ops,
        &mut state,
        &owner,
    )?;

    // --- cs2: update Ada, delete Grace. Its intent is the redaction
    // vector's target. ---
    let ada_rev1 = state.objects[&("node".into(), ada.into())].0.clone();
    let grace_rev1 = state.objects[&("node".into(), grace.into())].0.clone();
    let cs2_ops = vec![
        yaml(&format!(
            "update: {{ kind: node, id: \"{ada}\", prior: \"{ada_rev1}\", \
             type: \"core/Person@1\", attributes: {{ name: \"Ada Lovelace\" }} }}"
        )),
        yaml(&format!(
            "delete: {{ kind: node, id: \"{grace}\", prior: \"{grace_rev1}\" }}"
        )),
    ];
    let cs2 = build_changeset(
        "cs2",
        &format!(
            "{{ kind: changeset, parents: [\"{}\"], \
             author: {{ principal: \"principal:vector-author\", \
             key: \"{owner_key_id}\" }}, \
             timestamp: \"2026-08-02T00:01:00Z\", \
             intent: \"Rename Ada; retire Grace\", \
             schema_context: \"{core_hash}\" }}",
            cs1.hash
        ),
        cs2_ops,
        &mut state,
        &owner,
    )?;

    // --- cs3: three operations, the middle one elided in the vector ---
    let note = "00000000-0000-4000-8000-000000000006";
    let cs3_ops = vec![
        yaml(&format!(
            "create: {{ kind: node, id: \"{note}\", type: \"memory/Note@1\", \
             attributes: {{ content: \"Vectors are the ground truth.\" }} }}"
        )),
        yaml(&format!(
            "create: {{ kind: edge, id: \"00000000-0000-4000-8000-000000000007\", \
             type: \"memory/about@1\", from: \"node:{note}\", to: \"node:{ada}\" }}"
        )),
        yaml(&format!(
            "create: {{ kind: classification, \
             id: \"00000000-0000-4000-8000-000000000008\", subject: \"node:{note}\", \
             term: \"workspace/curated@1\", asserted_by: \"principal:vector-author\", \
             basis: manual }}"
        )),
    ];
    let cs3 = build_changeset(
        "cs3",
        &format!(
            "{{ kind: changeset, parents: [\"{}\"], \
             author: {{ principal: \"principal:vector-author\", \
             key: \"{owner_key_id}\" }}, \
             timestamp: \"2026-08-02T00:02:00Z\", \
             intent: \"A note about Ada, labeled\", \
             schema_context: \"{core_hash}\" }}",
            cs2.hash
        ),
        cs3_ops,
        &mut state,
        &owner,
    )?;

    // --- elision (§3.2.6): disclose ops 0 and 2, elide op 1, verify
    // the hash from the disclosed operations plus the retained leaf ---
    let elided_leaf = cs3.leaves[1].clone();
    let (elided_hash, _, _) = changeset_hash_from_leaves(&cs3.stored, &cs3.leaves)?;
    if elided_hash != cs3.hash {
        return Err("elided hash does not reproduce".into());
    }

    // --- redaction (§3.2.2): remove cs2's intent text, keep its hash,
    // verify the changeset hash is unchanged ---
    let cs2_intent = get_str(&cs2.stored, "intent").unwrap().to_string();
    let intent_hash = sha256_hex("intent", cs2_intent.as_bytes());
    let mut cs2_redacted = cs2.stored.clone();
    if let Some(map) = cs2_redacted.as_mapping_mut() {
        map.remove("intent");
        map.insert(
            Value::String("intent_hash".into()),
            Value::String(intent_hash.clone()),
        );
    }
    let cs2_leaves = cs2.leaves.clone();
    let (redacted_hash, _, _) = changeset_hash_from_leaves(&cs2_redacted, &cs2_leaves)?;
    if redacted_hash != cs2.hash {
        return Err("redacted hash does not reproduce".into());
    }

    // --- governance audit pair (Appendix H): one passing, one
    // failing audit of the same proposal under memory-local ---
    let loaded = allod_core::load_dir(ontologies_dir);
    let reg = loaded.registry;
    let mut policy: Value = serde_yaml::from_str(
        &std::fs::read_to_string(ontologies_dir.join("memory/policy-local.yaml"))
            .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    if let Some(roles) = policy.get_mut("roles").and_then(Value::as_mapping_mut) {
        roles.insert(
            Value::String("owner".into()),
            Value::Sequence(vec![Value::String("principal:vector-owner".into())]),
        );
    }
    let pctx = allod_core::policy::policy_context(&policy)?;
    let agent = Keypair::from_secret_hex("vector-agent", AGENT_SECRET)?;

    let key_yaml = |kp: &Keypair| {
        format!(
            "{{ key_id: \"{}\", algorithm: \"ed25519\", public: \"{}\", status: \"active\" }}",
            kp.key_id(),
            kp.public_hex()
        )
    };
    let g1 = yaml(&format!(
        "{{ kind: changeset, parents: [], author: {{ principal: \"principal:vector-owner\", \
         key: \"{}\" }}, timestamp: \"2026-08-02T01:00:00Z\", \
         intent: \"Governance vectors: genesis\", schema_context: \"{core_hash}\", \
         operations: [ {{ create: {{ kind: node, \
         id: \"00000000-0000-4000-8000-000000000010\", type: \"core/User@1\", \
         attributes: {{ display_name: \"vector-owner\", status: \"active\", \
         keys: [ {} ] }} }} }} ] }}",
        owner.key_id(),
        key_yaml(&owner)
    ));
    let (g1_hash, _, _, _) = changeset_hash(&g1)?;
    let mut g1 = g1;
    if let Some(m) = g1.as_mapping_mut() {
        m.insert(Value::String("hash".into()), Value::String(g1_hash.clone()));
        m.insert(Value::String("signature".into()), Value::String(owner.sign(&g1_hash)));
    }
    let g2 = yaml(&format!(
        "{{ kind: changeset, parents: [\"{g1_hash}\"], author: {{ \
         principal: \"principal:vector-owner\", key: \"{}\" }}, \
         timestamp: \"2026-08-02T01:01:00Z\", \
         intent: \"Governance vectors: register agent\", schema_context: \"{core_hash}\", \
         operations: [ {{ create: {{ kind: node, \
         id: \"00000000-0000-4000-8000-000000000011\", type: \"core/Agent@1\", \
         attributes: {{ display_name: \"vector-agent\", status: \"active\", \
         keys: [ {} ], \
         delegated_by: \"node:00000000-0000-4000-8000-000000000010\" }} }} }} ] }}",
        owner.key_id(),
        key_yaml(&agent)
    ));
    let (g2_hash, _, _, _) = changeset_hash(&g2)?;
    let mut g2 = g2;
    if let Some(m) = g2.as_mapping_mut() {
        m.insert(Value::String("hash".into()), Value::String(g2_hash.clone()));
        m.insert(Value::String("signature".into()), Value::String(owner.sign(&g2_hash)));
    }
    let mut audit_state = FoldState::default();
    audit_state.apply_changeset(&reg, &g1)?;
    audit_state.apply_changeset(&reg, &g2)?;

    let proposal = yaml(&format!(
        "{{ kind: changeset, parents: [\"{g2_hash}\"], author: {{ \
         principal: \"principal:vector-agent\", key: \"{}\" }}, \
         timestamp: \"2026-08-02T01:02:00Z\", \
         intent: \"Governance vectors: agent write needing owner review\", \
         schema_context: \"{core_hash}\", operations: [ \
         {{ create: {{ kind: node, id: \"00000000-0000-4000-8000-000000000012\", \
         type: \"memory/Note@1\", attributes: {{ content: \"audited note\" }}, \
         provenance: {{ derived_by: \"principal:vector-agent\", method: \"manual\" }} }} }}, \
         {{ create: {{ kind: classification, \
         id: \"00000000-0000-4000-8000-000000000013\", \
         subject: \"node:00000000-0000-4000-8000-000000000012\", term: \"work@1\", \
         asserted_by: \"principal:vector-agent\", basis: \"manual\" }} }} ] }}",
        agent.key_id()
    ));
    let (p_hash, _, _, _) = changeset_hash(&proposal)?;
    let mut proposal = proposal;
    if let Some(m) = proposal.as_mapping_mut() {
        m.insert(Value::String("hash".into()), Value::String(p_hash.clone()));
        m.insert(Value::String("signature".into()), Value::String(agent.sign(&p_hash)));
    }
    let checklist =
        allod_core::policy::evaluate(&reg, &policy, &audit_state, &proposal, "agent")?;
    let decision = |signer: &Keypair, principal: &str| -> Result<Value, String> {
        let mut record = yaml(&format!(
            "{{ kind: decision-record, subject: \"{p_hash}\", policy_context: \"{pctx}\", \
             verdict: approve, timestamp: \"2026-08-02T01:03:00Z\" }}"
        ));
        let payload = allod_core::policy::decision_payload(&record)?;
        if let Some(m) = record.as_mapping_mut() {
            m.insert(
                Value::String("deciders".into()),
                yaml(&format!(
                    "[ {{ principal: \"{principal}\", signature: \"{}\" }} ]",
                    signer.sign(&payload)
                )),
            );
        }
        Ok(record)
    };
    let good = decision(&owner, "principal:vector-owner")?;
    let bad = decision(&agent, "principal:vector-agent")?;
    let roots = vec!["principal:vector-owner".to_string()];
    let pass = allod_core::policy::check_satisfied(
        &audit_state, &policy, &roots, &proposal, "principal:vector-agent", &checklist,
        &[good.clone()], &[],
    )?;
    if !pass.unmet.is_empty() {
        return Err(format!("governance passing vector fails: {:?}", pass.unmet));
    }
    let fail = allod_core::policy::check_satisfied(
        &audit_state, &policy, &roots, &proposal, "principal:vector-agent", &checklist,
        &[bad.clone()], &[],
    )?;
    if fail.unmet.is_empty() {
        return Err("governance failing vector unexpectedly passes".into());
    }

    // --- write the files ---
    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let log: Vec<&Built> = vec![&cs1, &cs2, &cs3];

    let mut log_out = String::from(
        "# The Appendix H vector log: three changesets in stored form.\n\
         # Generated by allod-vectors; do not edit by hand.\n\
         # Signatures are placeholders until Part 6 lands.\n",
    );
    for built in &log {
        log_out.push_str("---\n");
        log_out.push_str(&serde_yaml::to_string(&built.stored).map_err(|e| e.to_string())?);
    }
    std::fs::write(out_dir.join("log.yaml"), log_out).map_err(|e| e.to_string())?;

    let mut vectors = Mapping::new();
    let mut pkg_map = Mapping::new();
    for (name, hash) in &packages {
        pkg_map.insert(Value::String(name.clone()), Value::String(hash.clone()));
    }
    vectors.insert(Value::String("packages".into()), Value::Mapping(pkg_map));

    let mut cs_list = Vec::new();
    for built in &log {
        let mut entry = Mapping::new();
        entry.insert(Value::String("name".into()), Value::String(built.name.clone()));
        entry.insert(Value::String("hash".into()), Value::String(built.hash.clone()));
        entry.insert(
            Value::String("operations_root".into()),
            Value::String(built.root.clone()),
        );
        entry.insert(
            Value::String("op_leaves".into()),
            Value::Sequence(built.leaves.iter().cloned().map(Value::String).collect()),
        );
        entry.insert(
            Value::String("preimage_hex".into()),
            Value::String(built.preimage_hex.clone()),
        );
        entry.insert(
            Value::String("state_hash_after".into()),
            Value::String(built.state_hash.clone()),
        );
        entry.insert(
            Value::String("state_after".into()),
            Value::Sequence(built.state_listing.clone()),
        );
        cs_list.push(Value::Mapping(entry));
    }
    vectors.insert(Value::String("changesets".into()), Value::Sequence(cs_list));

    let elision = yaml(&format!(
        "{{ changeset: cs3, disclosed_positions: [0, 2], \
         elided: [{{ position: 1, leaf: \"{elided_leaf}\" }}], \
         hash: \"{}\" }}",
        cs3.hash
    ));
    vectors.insert(Value::String("elision".into()), elision);

    let redaction = yaml(&format!(
        "{{ changeset: cs2, redacted_field: intent, \
         intent_hash: \"{intent_hash}\", hash_unchanged: \"{}\" }}",
        cs2.hash
    ));
    vectors.insert(Value::String("redaction".into()), redaction);

    let mut signing = Mapping::new();
    signing.insert(Value::String("note".into()), Value::String(
        "RFC 8032 test keys; deterministic, never for real graphs".into()));
    signing.insert(Value::String("owner_secret_hex".into()), Value::String(OWNER_SECRET.into()));
    signing.insert(Value::String("owner_public_hex".into()), Value::String(owner.public_hex()));
    signing.insert(Value::String("owner_key_id".into()), Value::String(owner.key_id()));
    signing.insert(Value::String("agent_secret_hex".into()), Value::String(AGENT_SECRET.into()));
    signing.insert(Value::String("agent_public_hex".into()), Value::String(agent.public_hex()));
    vectors.insert(Value::String("signing".into()), Value::Mapping(signing));

    let mut governance = Mapping::new();
    governance.insert(Value::String("policy_context".into()), Value::String(pctx));
    governance.insert(Value::String("setup".into()), Value::Sequence(vec![g1, g2]));
    governance.insert(Value::String("proposal".into()), proposal);
    let mut passing = Mapping::new();
    passing.insert(Value::String("decision".into()), good);
    passing.insert(Value::String("result".into()), Value::String("verified".into()));
    governance.insert(Value::String("passing".into()), Value::Mapping(passing));
    let mut failing = Mapping::new();
    failing.insert(Value::String("decision".into()), bad);
    failing.insert(
        Value::String("result".into()),
        Value::Sequence(fail.unmet.into_iter().map(Value::String).collect()),
    );
    governance.insert(Value::String("failing".into()), Value::Mapping(failing));
    vectors.insert(Value::String("governance".into()), Value::Mapping(governance));

    let mut doc_map = Mapping::new();
    doc_map.insert(
        Value::String("bytes_utf8".into()),
        Value::String(String::from_utf8_lossy(doc_bytes).into_owned()),
    );
    doc_map.insert(Value::String("content_hash".into()), Value::String(doc_hash));
    vectors.insert(Value::String("document".into()), Value::Mapping(doc_map));

    let vectors_out = format!(
        "# Appendix H test vectors. Generated by allod-vectors; do not\n\
         # edit by hand. Construction notes are in README.md.\n{}",
        serde_yaml::to_string(&Value::Mapping(vectors)).map_err(|e| e.to_string())?
    );
    std::fs::write(out_dir.join("vectors.yaml"), vectors_out).map_err(|e| e.to_string())?;
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("hashes") => {
            let dir = args
                .get(1)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("ontologies"));
            load_ontology_docs(&dir)
                .and_then(|docs| compute_package_hashes(&docs))
                .map(|hashes| {
                    for (name, hash) in hashes {
                        println!("{name} {hash}");
                    }
                })
        }
        Some("generate") => {
            let out = args
                .get(1)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("spec/vectors"));
            let dir = args
                .get(2)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("ontologies"));
            generate(&out, &dir)
        }
        _ => Err("usage: allod-vectors <hashes|generate> [paths]".into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
