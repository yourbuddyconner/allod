//! Federation (Part 9): peers, grants, share bundles, and governed
//! imports between two sovereign graphs, over any byte channel — here
//! a file.
//!
//! The bundle carries the grant, a signed checkpoint reference, and
//! the disclosed objects each with a Merkle path to the state hash
//! (§9.5, §5.4). The receiver verifies with the peer's keys and
//! admits imports under its own policy (§9.6, design principle 6).

use crate::{admit_or_hold, build_changeset, now_iso, s, short, uuid4};
use allod_core::get_str;
use allod_core::hash::{sha256_hex, verify_merkle_path};
use allod_core::model::revision_hash;
use allod_core::store::Graph;
use allod_core::{canonical_cbor, sign};
use serde_yaml::{Mapping, Value};
use std::fs;
use std::path::Path;

fn signed_payload(doc: &Value, domain: &str) -> Result<String, String> {
    let mut pre = doc.clone();
    if let Some(map) = pre.as_mapping_mut() {
        map.remove("signature");
    }
    Ok(sha256_hex(domain, &canonical_cbor(&pre)?))
}

pub fn peer_add(
    dir: &Path,
    name: &str,
    graph_id: &str,
    root_key_hex: &str,
    by: &str,
) -> Result<(), String> {
    let graph = Graph::open(dir)?;
    let kp = graph.load_key(by)?;
    let mut attrs = Mapping::new();
    attrs.insert(s("graph_id"), s(graph_id));
    attrs.insert(
        s("root_keys"),
        Value::Sequence(vec![s(root_key_hex)]),
    );
    attrs.insert(s("trust_basis"), s("out-of-band"));
    let mut node = Mapping::new();
    node.insert(s("kind"), s("node"));
    node.insert(s("id"), s(&uuid4()));
    node.insert(s("type"), s("core/Peer@1"));
    node.insert(s("attributes"), Value::Mapping(attrs));
    let mut op = Mapping::new();
    op.insert(s("create"), Value::Mapping(node));
    let (cs, hash) = build_changeset(
        &graph,
        &kp,
        &format!("Register peer {name} ({})", short(graph_id)),
        vec![Value::Mapping(op)],
    )?;
    admit_or_hold(&graph, by, &cs, &hash, vec![], false)?;
    Ok(())
}

/// Issue a grant (§9.4): a governed graph object authorizing
/// disclosure of a region to an audience. Returns the grant node id.
pub fn grant(dir: &Path, audience: &str, region: &str, by: &str) -> Result<String, String> {
    let graph = Graph::open(dir)?;
    let kp = graph.load_key(by)?;
    let grant_id = uuid4();
    let mut scope = Mapping::new();
    scope.insert(s("region"), s(region));
    let mut attrs = Mapping::new();
    attrs.insert(s("audience"), s(audience));
    attrs.insert(s("scope"), Value::Mapping(scope));
    attrs.insert(
        s("rights"),
        Value::Sequence(vec![s("state"), s("subscribe")]),
    );
    let mut node = Mapping::new();
    node.insert(s("kind"), s("node"));
    node.insert(s("id"), s(&grant_id));
    node.insert(s("type"), s("core/Grant@1"));
    node.insert(s("attributes"), Value::Mapping(attrs));
    let mut op = Mapping::new();
    op.insert(s("create"), Value::Mapping(node));
    let (cs, hash) = build_changeset(
        &graph,
        &kp,
        &format!("Grant region {region} to {}", short(audience)),
        vec![Value::Mapping(op)],
    )?;
    admit_or_hold(&graph, by, &cs, &hash, vec![], false)?;
    Ok(grant_id)
}

/// Revoke a grant: a governed deletion. Bytes already transferred
/// stay transferred; future bundle production is refused (§9.4).
pub fn revoke(dir: &Path, grant_id: &str, by: &str) -> Result<(), String> {
    let graph = Graph::open(dir)?;
    let kp = graph.load_key(by)?;
    let state = graph.fold()?;
    let obj = state
        .get_live("node", grant_id)
        .ok_or("grant not found or already revoked")?;
    let mut del = Mapping::new();
    del.insert(s("kind"), s("node"));
    del.insert(s("id"), s(grant_id));
    del.insert(s("prior"), s(&obj.rev));
    let mut op = Mapping::new();
    op.insert(s("delete"), Value::Mapping(del));
    let (cs, hash) = build_changeset(
        &graph,
        &kp,
        &format!("Revoke grant {}", short(grant_id)),
        vec![Value::Mapping(op)],
    )?;
    admit_or_hold(&graph, by, &cs, &hash, vec![], false)?;
    Ok(())
}

/// Produce a share bundle for a grant (§9.5): checkpoint reference,
/// disclosed objects with membership proofs, and the schema.
pub fn bundle(dir: &Path, grant_id: &str, out: &Path, by: &str) -> Result<(), String> {
    let graph = Graph::open(dir)?;
    let kp = graph.load_key(by)?;
    let reg = graph.registry()?;
    let state = graph.fold()?;

    let grant_obj = state
        .get_live("node", grant_id)
        .ok_or("no live grant with that id — issued? revoked? (§9.4)")?;
    let grant_attrs = grant_obj.content.get("attributes").ok_or("grant has no attributes")?;
    let region = grant_attrs
        .get("scope")
        .and_then(|sc| get_str(sc, "region"))
        .ok_or("grant scope needs a region")?
        .to_string();

    // The disclosed set: nodes classified under the region (ancestor
    // closure, §4.1), their classifications, and edges between them.
    let entries = state.entries();
    let leaves: Vec<String> = entries
        .iter()
        .map(|e| Ok::<String, String>(sha256_hex("state-leaf", &canonical_cbor(e)?)))
        .collect::<Result<_, _>>()?;
    let state_hash = state.state_hash()?;

    let mut disclosed_nodes: Vec<String> = Vec::new();
    for ((kind, id), obj) in &state.objects {
        if kind != "node" || obj.deleted {
            continue;
        }
        let node_ref = format!("node:{id}");
        let in_scope = state.classifications_of(&node_ref).iter().any(|t| {
            reg.term_closure(t).contains(allod_core::bare(&region))
        });
        if in_scope {
            disclosed_nodes.push(id.clone());
        }
    }
    let mut objects = Vec::new();
    let mut disclose = |kind: &str, id: &str| -> Result<(), String> {
        let index = state
            .objects
            .keys()
            .position(|(k, i)| k == kind && i == id)
            .ok_or("object index")?;
        let obj = &state.objects[&(kind.to_string(), id.to_string())];
        let proof = allod_core::hash::merkle_path(&leaves, index, "state-node")
            .ok_or("proof generation")?;
        let mut entry = Mapping::new();
        entry.insert(s("entry"), entries[index].clone());
        entry.insert(s("content"), obj.content.clone());
        entry.insert(
            s("proof"),
            Value::Sequence(
                proof
                    .into_iter()
                    .map(|(sib, left)| {
                        let mut step = Mapping::new();
                        step.insert(s("sibling"), s(&sib));
                        step.insert(s("left"), Value::Bool(left));
                        Value::Mapping(step)
                    })
                    .collect(),
            ),
        );
        objects.push(Value::Mapping(entry));
        Ok(())
    };
    for id in &disclosed_nodes {
        disclose("node", id)?;
    }
    for ((kind, id), obj) in &state.objects {
        if obj.deleted {
            continue;
        }
        if kind == "classification" {
            let subject = get_str(&obj.content, "subject").unwrap_or("");
            if disclosed_nodes
                .iter()
                .any(|n| subject == format!("node:{n}"))
            {
                disclose("classification", id)?;
            }
        } else if kind == "edge" {
            let from = get_str(&obj.content, "from").unwrap_or("");
            let to = get_str(&obj.content, "to").unwrap_or("");
            let covered = |r: &str| {
                disclosed_nodes.iter().any(|n| r == format!("node:{n}"))
            };
            if covered(from) && covered(to) {
                disclose("edge", id)?;
            }
        }
    }

    // Signed checkpoint reference (§3.2.5, §9.5).
    let mut checkpoint = Mapping::new();
    checkpoint.insert(s("kind"), s("checkpoint"));
    checkpoint.insert(s("revision"), s(&graph.head()?.unwrap_or_default()));
    checkpoint.insert(s("state_hash"), s(&state_hash));
    checkpoint.insert(s("timestamp"), s(&now_iso()));
    checkpoint.insert(s("signer"), s(&format!("principal:{by}")));
    let mut checkpoint = Value::Mapping(checkpoint);
    let payload = signed_payload(&checkpoint, "checkpoint")?;
    if let Some(map) = checkpoint.as_mapping_mut() {
        map.insert(s("signature"), s(&kp.sign(&payload)));
    }

    let mut doc = Mapping::new();
    doc.insert(s("kind"), s("share-bundle"));
    doc.insert(
        s("graph_id"),
        s(get_str(&graph.meta()?, "graph_id").unwrap_or("")),
    );
    let mut grant_ref = grant_attrs.clone();
    if let Some(map) = grant_ref.as_mapping_mut() {
        map.insert(s("id"), s(grant_id));
    }
    doc.insert(s("grant"), grant_ref);
    doc.insert(s("checkpoint"), checkpoint);
    doc.insert(s("objects"), Value::Sequence(objects.clone()));
    let mut schema = Mapping::new();
    for (name, sdoc) in graph.schema_docs()? {
        schema.insert(s(&name), sdoc);
    }
    doc.insert(s("schema"), Value::Mapping(schema));
    fs::write(
        out,
        serde_yaml::to_string(&Value::Mapping(doc)).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    println!(
        "  ✓ bundle: {} objects in region {region}, checkpoint {} (state {})",
        objects.len(),
        short(&graph.head()?.unwrap_or_default()),
        short(&state_hash)
    );
    Ok(())
}

/// Verify a bundle against the peer's keys and optionally import one
/// object through local admission (§9.6). Returns the proposal hash
/// when the import is held by local policy.
pub fn import(
    dir: &Path,
    bundle_path: &Path,
    by: &str,
    import_id: Option<&str>,
) -> Result<Option<String>, String> {
    let graph = Graph::open(dir)?;
    let state = graph.fold()?;
    let bundle: Value = serde_yaml::from_str(
        &fs::read_to_string(bundle_path).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let source_graph = get_str(&bundle, "graph_id").ok_or("bundle needs graph_id")?;

    // The peer record supplies the keys (§9.3).
    let peer_key = state
        .objects
        .iter()
        .find_map(|((kind, _), obj)| {
            if kind != "node" || obj.deleted {
                return None;
            }
            let attrs = obj.content.get("attributes")?;
            if get_str(&obj.content, "type").map(allod_core::bare) == Some("core/Peer")
                && get_str(attrs, "graph_id") == Some(source_graph)
            {
                attrs
                    .get("root_keys")?
                    .as_sequence()?
                    .first()?
                    .as_str()
                    .map(String::from)
            } else {
                None
            }
        })
        .ok_or_else(|| format!("no peer record for graph {}", short(source_graph)))?;

    // Checkpoint signature against the peer's root key.
    let checkpoint = bundle.get("checkpoint").ok_or("bundle needs checkpoint")?;
    let payload = signed_payload(checkpoint, "checkpoint")?;
    sign::verify(
        &peer_key,
        &payload,
        get_str(checkpoint, "signature").unwrap_or(""),
    )
    .map_err(|e| format!("checkpoint signature: {e}"))?;
    let state_hash = get_str(checkpoint, "state_hash").ok_or("checkpoint needs state_hash")?;

    // Grant audience must be this graph (or public).
    let my_id = get_str(&graph.meta()?, "graph_id").unwrap_or("").to_string();
    let audience = bundle
        .get("grant")
        .and_then(|g| get_str(g, "audience"))
        .unwrap_or("");
    if audience != my_id && audience != "public" {
        return Err(format!(
            "grant audience {} is not this graph ({})",
            short(audience),
            short(&my_id)
        ));
    }

    // Every disclosed object: content matches its revision, and the
    // membership proof reaches the checkpoint state hash (§5.4).
    let objects = bundle
        .get("objects")
        .and_then(Value::as_sequence)
        .ok_or("bundle needs objects")?;
    for obj in objects {
        let entry = obj.get("entry").ok_or("object needs entry")?;
        let content = obj.get("content").ok_or("object needs content")?;
        let rev = get_str(entry, "rev").unwrap_or("");
        if revision_hash(content)? != rev {
            return Err(format!("object {} content does not match its revision",
                get_str(entry, "id").unwrap_or("?")));
        }
        let leaf = sha256_hex("state-leaf", &canonical_cbor(entry)?);
        let proof: Vec<(String, bool)> = obj
            .get("proof")
            .and_then(Value::as_sequence)
            .map(|seq| {
                seq.iter()
                    .filter_map(|step| {
                        Some((
                            get_str(step, "sibling")?.to_string(),
                            step.get("left")?.as_bool()?,
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        if !verify_merkle_path(&leaf, &proof, state_hash, "state-node") {
            return Err(format!(
                "membership proof fails for {}",
                get_str(entry, "id").unwrap_or("?")
            ));
        }
    }
    println!(
        "  ✓ bundle verified: {} objects prove membership in state {} of peer {}",
        objects.len(),
        short(state_hash),
        short(source_graph)
    );

    let Some(import_id) = import_id else { return Ok(None) };
    let (entry, content) = objects
        .iter()
        .find_map(|obj| {
            let entry = obj.get("entry")?;
            let content = obj.get("content")?;
            (get_str(entry, "id") == Some(import_id)).then_some((entry, content))
        })
        .ok_or("import target not in bundle")?;
    let rev = get_str(entry, "rev").unwrap_or("");

    // Import (§9.6): the local principal authors the proposal, and
    // lineage records the foreign object by allod: reference.
    let mut payload = content.clone();
    if let Some(map) = payload.as_mapping_mut() {
        let mut prov = Mapping::new();
        prov.insert(
            s("derived_from"),
            Value::Sequence(vec![s(&format!(
                "allod:{source_graph}/{import_id}@{rev}"
            ))]),
        );
        prov.insert(s("derived_by"), s(&format!("principal:{by}")));
        prov.insert(s("method"), s("deterministic"));
        prov.insert(s("tool"), s("allod-import@0.1"));
        map.insert(s("provenance"), Value::Mapping(prov));
    }
    let mut op = Mapping::new();
    op.insert(s("create"), payload);
    let kp = graph.load_key(by)?;
    let (cs, hash) = build_changeset(
        &graph,
        &kp,
        &format!("Import {} from peer {}", short(import_id), short(source_graph)),
        vec![Value::Mapping(op)],
    )?;
    let admitted = admit_or_hold(&graph, by, &cs, &hash, vec![], false)?;
    println!(
        "      lineage: derived_from allod:{}/{}@{}",
        short(source_graph),
        short(import_id),
        short(rev)
    );
    Ok(if admitted { None } else { Some(hash) })
}
