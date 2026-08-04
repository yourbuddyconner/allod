//! Federation (Part 9): peers, grants, share bundles, and governed
//! imports between two sovereign graphs, over any byte channel.
//!
//! The bundle carries the grant, a signed checkpoint reference, and
//! the disclosed objects each with a Merkle path to the state hash
//! (§9.5, §5.4). The receiver verifies with the peer's keys and
//! admits imports under its own policy (§9.6, design principle 6).
//!
//! This module is pure (no filesystem, no printing); the CLI shim in
//! `crates/allod/src/fed.rs` handles file I/O and stdout output.

use allod_core::get_str;
use allod_core::hash::{sha256_hex, verify_merkle_path};
use allod_core::model::revision_hash;
use allod_core::store::Graph;
use allod_core::{canonical_cbor, sign};
use serde_yaml::{Mapping, Value};

use crate::ops::{admit_or_hold, build_changeset, now_iso, short, uuid4, Admission};
use crate::AllodError;

fn s(v: &str) -> Value {
    Value::String(v.to_string())
}

fn signed_payload(doc: &Value, domain: &str) -> Result<String, AllodError> {
    let mut pre = doc.clone();
    if let Some(map) = pre.as_mapping_mut() {
        map.remove("signature");
    }
    Ok(sha256_hex(domain, &canonical_cbor(&pre)?))
}

/// Register a peer graph by out-of-band key exchange (§9.3).
pub fn peer_add(
    graph: &Graph,
    name: &str,
    graph_id: &str,
    root_key_hex: &str,
    by: &str,
) -> Result<(), AllodError> {
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
        graph,
        &kp,
        &format!("Register peer {name} ({})", short(graph_id)),
        vec![Value::Mapping(op)],
    )?;
    admit_or_hold(graph, by, &cs, &hash, vec![])?;
    Ok(())
}

/// Issue a grant (§9.4): a governed graph object authorizing
/// disclosure of a region to an audience. Returns the grant node id.
pub fn grant(graph: &Graph, audience: &str, region: &str, by: &str) -> Result<String, AllodError> {
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
        graph,
        &kp,
        &format!("Grant region {region} to {}", short(audience)),
        vec![Value::Mapping(op)],
    )?;
    admit_or_hold(graph, by, &cs, &hash, vec![])?;
    Ok(grant_id)
}

/// Revoke a grant: a governed deletion. Bytes already transferred
/// stay transferred; future bundle production is refused (§9.4).
pub fn revoke(graph: &Graph, grant_id: &str, by: &str) -> Result<(), AllodError> {
    let kp = graph.load_key(by)?;
    let state = graph.fold()?;
    let obj = state
        .get_live("node", grant_id)
        .ok_or_else(|| AllodError::NotFound("grant not found or already revoked".into()))?;
    let mut del = Mapping::new();
    del.insert(s("kind"), s("node"));
    del.insert(s("id"), s(grant_id));
    del.insert(s("prior"), s(&obj.rev));
    let mut op = Mapping::new();
    op.insert(s("delete"), Value::Mapping(del));
    let (cs, hash) = build_changeset(
        graph,
        &kp,
        &format!("Revoke grant {}", short(grant_id)),
        vec![Value::Mapping(op)],
    )?;
    admit_or_hold(graph, by, &cs, &hash, vec![])?;
    Ok(())
}

/// Produce a share bundle for a grant (§9.5): checkpoint reference,
/// disclosed objects with membership proofs, and the schema.
///
/// Returns a pure `Value` (no filesystem writes, no printing) — this
/// is the WASM-safe surface. The CLI shim writes the result to disk
/// and prints the confirmation line.
pub fn make_bundle(graph: &Graph, grant_id: &str, by: &str) -> Result<Value, AllodError> {
    let kp = graph.load_key(by)?;
    let reg = graph.registry()?;
    let state = graph.fold()?;

    let grant_obj = state
        .get_live("node", grant_id)
        .ok_or_else(|| AllodError::NotFound(
            "no live grant with that id — issued? revoked? (§9.4)".into(),
        ))?;
    let grant_attrs = grant_obj
        .content
        .get("attributes")
        .ok_or_else(|| AllodError::Other("grant has no attributes".into()))?;
    let region = grant_attrs
        .get("scope")
        .and_then(|sc| get_str(sc, "region"))
        .ok_or_else(|| AllodError::Other("grant scope needs a region".into()))?
        .to_string();

    // The disclosed set: nodes classified under the region (ancestor
    // closure, §4.1), their classifications, and edges between them.
    let entries = state.entries();
    let leaves: Vec<String> = entries
        .iter()
        .map(|e| Ok::<String, AllodError>(sha256_hex("state-leaf", &canonical_cbor(e)?)))
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

    let mut objects: Vec<Value> = Vec::new();
    let mut disclose = |kind: &str, id: &str| -> Result<(), AllodError> {
        let index = state
            .objects
            .keys()
            .position(|(k, i)| k == kind && i == id)
            .ok_or_else(|| AllodError::Other("object index".into()))?;
        let obj = &state.objects[&(kind.to_string(), id.to_string())];
        let proof = allod_core::hash::merkle_path(&leaves, index, "state-node")
            .ok_or_else(|| AllodError::Other("proof generation".into()))?;
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
    doc.insert(s("objects"), Value::Sequence(objects));
    let mut schema = Mapping::new();
    for (name, sdoc) in graph.schema_docs()? {
        schema.insert(s(&name), sdoc);
    }
    doc.insert(s("schema"), Value::Mapping(schema));

    Ok(Value::Mapping(doc))
}

/// Verify a bundle against the peer's keys and import one object
/// through local admission (§9.6). Returns the `Admission` outcomes
/// for the imported object — the CLI shim prints them.
///
/// `import_id` is the node id to import from the bundle.
pub fn import(
    graph: &Graph,
    bundle: &Value,
    by: &str,
    import_id: &str,
) -> Result<Vec<Admission>, AllodError> {
    let state = graph.fold()?;
    let source_graph = get_str(bundle, "graph_id")
        .ok_or_else(|| AllodError::Other("bundle needs graph_id".into()))?;

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
        .ok_or_else(|| AllodError::NotFound(
            format!("no peer record for graph {}", short(source_graph)),
        ))?;

    // Checkpoint signature against the peer's root key.
    let checkpoint = bundle
        .get("checkpoint")
        .ok_or_else(|| AllodError::Other("bundle needs checkpoint".into()))?;
    let payload = signed_payload(checkpoint, "checkpoint")?;
    sign::verify(
        &peer_key,
        &payload,
        get_str(checkpoint, "signature").unwrap_or(""),
    )
    .map_err(|e| AllodError::SignatureInvalid(format!("checkpoint signature: {e}")))?;
    let state_hash = get_str(checkpoint, "state_hash")
        .ok_or_else(|| AllodError::Other("checkpoint needs state_hash".into()))?;

    // Grant audience must be this graph (or public).
    let my_id = get_str(&graph.meta()?, "graph_id").unwrap_or("").to_string();
    let audience = bundle
        .get("grant")
        .and_then(|g| get_str(g, "audience"))
        .unwrap_or("");
    if audience != my_id && audience != "public" {
        return Err(AllodError::Other(format!(
            "grant audience {} is not this graph ({})",
            short(audience),
            short(&my_id)
        )));
    }

    // Every disclosed object: content matches its revision, and the
    // membership proof reaches the checkpoint state hash (§5.4).
    let objects = bundle
        .get("objects")
        .and_then(Value::as_sequence)
        .ok_or_else(|| AllodError::Other("bundle needs objects".into()))?;
    for obj in objects {
        let entry = obj
            .get("entry")
            .ok_or_else(|| AllodError::Other("object needs entry".into()))?;
        let content = obj
            .get("content")
            .ok_or_else(|| AllodError::Other("object needs content".into()))?;
        let rev = get_str(entry, "rev").unwrap_or("");
        if revision_hash(content)? != rev {
            return Err(AllodError::HashMismatch(format!(
                "object {} content does not match its revision",
                get_str(entry, "id").unwrap_or("?")
            )));
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
            return Err(AllodError::HashMismatch(format!(
                "membership proof fails for {}",
                get_str(entry, "id").unwrap_or("?")
            )));
        }
    }

    // Find the target object in the bundle.
    let (entry, content) = objects
        .iter()
        .find_map(|obj| {
            let entry = obj.get("entry")?;
            let content = obj.get("content")?;
            (get_str(entry, "id") == Some(import_id)).then_some((entry, content))
        })
        .ok_or_else(|| AllodError::NotFound("import target not in bundle".into()))?;
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
        graph,
        &kp,
        &format!("Import {} from peer {}", short(import_id), short(source_graph)),
        vec![Value::Mapping(op)],
    )?;
    let admission = admit_or_hold(graph, by, &cs, &hash, vec![])?;
    Ok(vec![admission])
}
