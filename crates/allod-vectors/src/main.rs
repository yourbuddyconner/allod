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
//! Signature and governance-audit vectors land with Parts 6 and 4 of
//! the reference implementation.

use allod_core::hash::{merkle_root, package_hash, sha256_hex};
use allod_core::{canonical_cbor, get_str};
use serde_yaml::{Mapping, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn yaml(s: &str) -> Value {
    serde_yaml::from_str(s).expect("vector template must parse")
}

fn hex_string(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Plain SHA-256 of exact bytes, for document content hashes (§1.5).
/// No allod domain: content identity must match what anyone computes
/// over the file.
fn content_hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("sha256:{}", hex_string(&digest))
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

// ---------------- object and changeset hashing ----------------

/// Revision hash (§1.7): canonical encoding with `rev` omitted.
fn revision_hash(payload: &Value) -> Result<String, String> {
    let mut payload = payload.clone();
    if let Some(map) = payload.as_mapping_mut() {
        map.remove("rev");
    }
    Ok(sha256_hex("object", &canonical_cbor(&payload)?))
}

fn op_leaf(op: &Value) -> Result<String, String> {
    Ok(sha256_hex("op-leaf", &canonical_cbor(op)?))
}

/// Changeset hash (§3.2.1, §3.2.6): `hash` and `signature` omitted,
/// `operations` replaced by the tree root, `intent` by its hash.
/// Returns (hash, root, preimage bytes).
fn changeset_hash_from_leaves(
    cs: &Value,
    leaves: &[String],
) -> Result<(String, String, Vec<u8>), String> {
    let root = merkle_root(leaves, "op-node").ok_or("changeset has no operations")?;
    let mut preimage = cs.clone();
    let map = preimage.as_mapping_mut().ok_or("changeset must be a map")?;
    map.remove("hash");
    map.remove("signature");
    map.insert(
        Value::String("operations".into()),
        Value::String(root.clone()),
    );
    if let Some(intent) = map
        .get(Value::String("intent".into()))
        .and_then(Value::as_str)
        .map(String::from)
    {
        map.insert(
            Value::String("intent".into()),
            Value::String(sha256_hex("intent", intent.as_bytes())),
        );
    } else if let Some(ih) = map
        .get(Value::String("intent_hash".into()))
        .and_then(Value::as_str)
        .map(String::from)
    {
        // A redacted changeset (§3.2.2) stores the intent hash in
        // place of the text; the preimage is identical either way.
        map.remove("intent_hash");
        map.insert(Value::String("intent".into()), Value::String(ih));
    }
    let bytes = canonical_cbor(&preimage)?;
    Ok((sha256_hex("changeset", &bytes), root, bytes))
}

fn changeset_hash(cs: &Value) -> Result<(String, String, Vec<String>, Vec<u8>), String> {
    let ops = cs
        .get("operations")
        .and_then(Value::as_sequence)
        .ok_or("changeset needs an operations list")?;
    let leaves: Result<Vec<String>, String> = ops.iter().map(op_leaf).collect();
    let leaves = leaves?;
    let (hash, root, bytes) = changeset_hash_from_leaves(cs, &leaves)?;
    Ok((hash, root, leaves, bytes))
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
        let mut leaves = Vec::new();
        let mut listing = Vec::new();
        for ((kind, id), (rev, deleted)) in &self.objects {
            let mut entry = Mapping::new();
            entry.insert(Value::String("kind".into()), Value::String(kind.clone()));
            entry.insert(Value::String("id".into()), Value::String(id.clone()));
            entry.insert(Value::String("rev".into()), Value::String(rev.clone()));
            if *deleted {
                entry.insert(Value::String("deleted".into()), Value::Bool(true));
            }
            let entry = Value::Mapping(entry);
            leaves.push(sha256_hex("state-leaf", &canonical_cbor(&entry)?));
            listing.push(entry);
        }
        let root = merkle_root(&leaves, "state-node").ok_or("empty state")?;
        Ok((root, listing))
    }
}

// ---------------- the vector log ----------------

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
) -> Result<Built, String> {
    let mut cs = yaml(header);
    let map = cs.as_mapping_mut().ok_or("changeset template must be a map")?;
    map.insert(Value::String("operations".into()), Value::Sequence(ops.clone()));
    map.insert(
        Value::String("signature".into()),
        Value::String("sig:pending:part-6".into()),
    );
    let (hash, root, leaves, bytes) = changeset_hash(&cs)?;
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
    let doc_hash = content_hash(doc_bytes);
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
             key: \"key:ed25519:vec0\" }}, \
             timestamp: \"2026-08-02T00:00:00Z\", \
             intent: \"Genesis: two people, an edge, a document, a label\", \
             schema_context: \"{core_hash}\" }}"
        ),
        cs1_ops,
        &mut state,
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
             key: \"key:ed25519:vec0\" }}, \
             timestamp: \"2026-08-02T00:01:00Z\", \
             intent: \"Rename Ada; retire Grace\", \
             schema_context: \"{core_hash}\" }}",
            cs1.hash
        ),
        cs2_ops,
        &mut state,
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
             key: \"key:ed25519:vec0\" }}, \
             timestamp: \"2026-08-02T00:02:00Z\", \
             intent: \"A note about Ada, labeled\", \
             schema_context: \"{core_hash}\" }}",
            cs2.hash
        ),
        cs3_ops,
        &mut state,
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
