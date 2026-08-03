//! The markdown bundle binding (§7.2): export a graph's nodes as a
//! directory of markdown files with YAML front matter, verify the
//! round trip, and re-ingest hand edits as proposals.
//!
//! Declared losses (§7.2): history, classification object envelopes
//! (terms survive in front matter), edge envelopes (typed links
//! survive), and documents. Re-ingest therefore reconstructs and
//! verifies the node set; a modified node file becomes an `update`
//! proposal through ordinary admission.

use crate::{admit_or_hold, build_changeset, s, short};
use allod_core::get_str;
use allod_core::model::revision_hash;
use allod_core::store::Graph;
use serde_yaml::{Mapping, Value};
use std::fs;
use std::path::Path;

/// The attribute the node's type marks `long_text: true` (§2.1).
fn long_text_attr(reg: &allod_core::Registry, type_ref: &str) -> Option<String> {
    let (pkg, name, _) = reg.resolve_type(type_ref, None)?;
    reg.collected_attrs(&pkg, &name)
        .into_iter()
        .find(|(_, adef)| adef.get("long_text").and_then(Value::as_bool) == Some(true))
        .map(|(aname, _)| aname)
}

pub fn export(graph_dir: &Path, out: &Path) -> Result<(), String> {
    let graph = Graph::open(graph_dir)?;
    let reg = graph.registry()?;
    let state = graph.fold()?;
    let state_hash = state.state_hash()?;
    fs::create_dir_all(out).map_err(|e| e.to_string())?;

    let mut count = 0usize;
    for ((kind, id), obj) in &state.objects {
        if kind != "node" || obj.deleted || obj.redacted {
            continue;
        }
        let node_ref = format!("node:{id}");
        let mut terms = state.classifications_of(&node_ref);
        terms.sort();
        let primary = terms
            .first()
            .map(|t| allod_core::bare(t).to_string())
            .unwrap_or_else(|| "_unclassified".into());
        let dir = out.join(&primary);
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

        let type_ref = get_str(&obj.content, "type").unwrap_or("").to_string();
        let long_attr = long_text_attr(&reg, &type_ref);
        let mut front = Mapping::new();
        front.insert(s("id"), s(id));
        front.insert(s("rev"), s(&obj.rev));
        front.insert(s("type"), s(&type_ref));
        if !terms.is_empty() {
            front.insert(
                s("classifications"),
                Value::Sequence(terms.iter().map(|t| s(t)).collect()),
            );
        }
        let mut body = String::new();
        if let Some(attrs) = obj.content.get("attributes").and_then(Value::as_mapping) {
            let mut kept = Mapping::new();
            for (aname, aval) in attrs {
                if long_attr.as_deref() == aname.as_str() {
                    body = aval.as_str().unwrap_or("").to_string();
                } else {
                    kept.insert(aname.clone(), aval.clone());
                }
            }
            front.insert(s("attributes"), Value::Mapping(kept));
        }
        if let Some(prov) = obj.content.get("provenance") {
            front.insert(s("provenance"), prov.clone());
        }
        // Outbound edges as typed links (declared-lossy: edge
        // envelopes do not survive).
        let mut links = Vec::new();
        for ((ekind, _), edge) in &state.objects {
            if ekind == "edge"
                && !edge.deleted
                && get_str(&edge.content, "from") == Some(node_ref.as_str())
            {
                let mut link = Mapping::new();
                link.insert(s("type"), s(get_str(&edge.content, "type").unwrap_or("")));
                link.insert(s("to"), s(get_str(&edge.content, "to").unwrap_or("")));
                links.push(Value::Mapping(link));
            }
        }
        if !links.is_empty() {
            front.insert(s("links"), Value::Sequence(links));
        }

        let front_text =
            serde_yaml::to_string(&Value::Mapping(front)).map_err(|e| e.to_string())?;
        let file = format!("---\n{front_text}---\n\n{}\n", body.trim_end_matches('\n'));
        fs::write(dir.join(format!("{id}.md")), file).map_err(|e| e.to_string())?;
        count += 1;
    }

    // Bundle-level .allod/: the manifest and the schema projection.
    let meta_dir = out.join(".allod");
    fs::create_dir_all(&meta_dir).map_err(|e| e.to_string())?;
    let mut manifest = Mapping::new();
    manifest.insert(s("state_hash"), s(&state_hash));
    manifest.insert(s("head"), s(&graph.head()?.unwrap_or_default()));
    manifest.insert(
        s("declared_losses"),
        Value::Sequence(
            ["history", "classification-envelopes", "edge-envelopes", "documents"]
                .iter()
                .map(|l| s(l))
                .collect(),
        ),
    );
    fs::write(
        meta_dir.join("manifest.yaml"),
        serde_yaml::to_string(&Value::Mapping(manifest)).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let schema_out = meta_dir.join("schema");
    fs::create_dir_all(&schema_out).map_err(|e| e.to_string())?;
    for (name, doc) in graph.schema_docs()? {
        fs::write(
            schema_out.join(format!("{name}.yaml")),
            serde_yaml::to_string(&doc).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }
    println!("  ✓ exported {count} nodes (state {})", short(&state_hash));
    Ok(())
}

fn parse_file(text: &str) -> Result<(Value, String), String> {
    let rest = text
        .strip_prefix("---\n")
        .ok_or("file lacks front matter")?;
    let (front, body) = rest.split_once("\n---\n").ok_or("unterminated front matter")?;
    let front: Value = serde_yaml::from_str(front).map_err(|e| e.to_string())?;
    Ok((front, body.trim().to_string()))
}

/// Rebuild a node payload from a bundle file and report whether it
/// matches its recorded revision. Returns (payload, front rev,
/// recomputed rev).
fn reconstruct(
    reg: &allod_core::Registry,
    front: &Value,
    body: &str,
) -> Result<(Value, String, String), String> {
    let id = get_str(front, "id").ok_or("front matter needs id")?;
    let type_ref = get_str(front, "type").ok_or("front matter needs type")?;
    let front_rev = get_str(front, "rev").ok_or("front matter needs rev")?.to_string();
    let mut attrs = front
        .get("attributes")
        .and_then(Value::as_mapping)
        .cloned()
        .unwrap_or_default();
    if let Some(long) = long_text_attr(reg, type_ref) {
        attrs.insert(s(&long), s(body));
    }
    let mut payload = Mapping::new();
    payload.insert(s("kind"), s("node"));
    payload.insert(s("id"), s(id));
    payload.insert(s("type"), s(type_ref));
    payload.insert(s("attributes"), Value::Mapping(attrs));
    if let Some(prov) = front.get("provenance") {
        payload.insert(s("provenance"), prov.clone());
    }
    let payload = Value::Mapping(payload);
    let recomputed = revision_hash(&payload)?;
    Ok((payload, front_rev, recomputed))
}

pub fn import(graph_dir: &Path, bundle: &Path, as_principal: &str) -> Result<(), String> {
    let graph = Graph::open(graph_dir)?;
    let reg = graph.registry()?;
    let state = graph.fold()?;
    let manifest: Value = serde_yaml::from_str(
        &fs::read_to_string(bundle.join(".allod/manifest.yaml")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let manifest_hash = get_str(&manifest, "state_hash").unwrap_or("").to_string();

    let mut files = Vec::new();
    collect_md(bundle, &mut files);
    let mut unchanged = 0usize;
    let mut updates: Vec<Value> = Vec::new();
    for path in files {
        let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let (front, body) = parse_file(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let (payload, front_rev, recomputed) = reconstruct(&reg, &front, &body)?;
        if front_rev == recomputed {
            unchanged += 1;
            continue;
        }
        // A hand edit (§7.2): the file re-enters as an update
        // proposal against the revision the bundle recorded.
        let mut update = payload;
        if let Some(map) = update.as_mapping_mut() {
            map.insert(s("prior"), s(&front_rev));
        }
        let mut op = Mapping::new();
        op.insert(s("update"), update);
        updates.push(Value::Mapping(op));
    }

    if updates.is_empty() {
        let current = state.state_hash()?;
        if current == manifest_hash {
            println!(
                "  ✓ round trip verified: {unchanged} nodes unchanged, state {} matches \
                 the bundle manifest (§7.4)",
                short(&current)
            );
        } else {
            println!(
                "  ⚠ bundle is unmodified but the graph has moved on: bundle state {}, \
                 graph state {}",
                short(&manifest_hash),
                short(&current)
            );
        }
        return Ok(());
    }
    let kp = graph.load_key(as_principal)?;
    let n = updates.len();
    let (cs, hash) = build_changeset(
        &graph,
        &kp,
        &format!("Bundle re-ingest: {n} edited file(s)"),
        updates,
    )?;
    println!("  {unchanged} unchanged, {n} edited — the edit enters admission (§7.2):");
    admit_or_hold(&graph, as_principal, &cs, &hash, vec![], false)?;
    Ok(())
}

fn collect_md(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            if path.file_name().and_then(|n| n.to_str()) != Some(".allod") {
                collect_md(&path, out);
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            out.push(path);
        }
    }
}
