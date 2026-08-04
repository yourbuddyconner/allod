//! The markdown bundle binding (§7.2): export a graph's nodes as a
//! directory of markdown files with YAML front matter, verify the
//! round trip, and re-ingest hand edits as proposals.
//!
//! Declared losses (§7.2): history, classification object envelopes
//! (terms survive in front matter), edge envelopes (typed links
//! survive), and documents. Re-ingest therefore reconstructs and
//! verifies the node set; a modified node file becomes an `update`
//! proposal through ordinary admission.

use allod_core::get_str;
use allod_core::model::revision_hash;
use allod_core::store::Graph;
use serde_yaml::{Mapping, Value};
use std::fs;
use std::path::{Path, PathBuf};

use crate::ops::{admit_or_hold, build_changeset, Admission};
use crate::AllodError;

// ---- Public result types ----

/// Returned by `export`: the number of node files written and the
/// state hash encoded in the bundle manifest.
pub struct ExportReport {
    pub files: usize,
    pub state_hash: String,
}

/// Returned by `import`: admission outcomes for any edited files that
/// were re-ingested, and files that could not be parsed.
pub struct ImportReport {
    pub admissions: Vec<Admission>,
    /// Number of files that were unchanged (rev matched).
    pub unchanged: usize,
    /// Number of edited files submitted to admission (all in one batch changeset).
    pub edited_files: usize,
    /// Files that failed to parse (path, error message).
    pub skipped: Vec<(PathBuf, String)>,
    /// State hash from the bundle manifest (for round-trip verification).
    pub manifest_hash: String,
}

// ---- Internal helpers ----

fn s(v: &str) -> Value {
    Value::String(v.to_string())
}

/// The attribute the node's type marks `long_text: true` (§2.1).
fn long_text_attr(reg: &allod_core::Registry, type_ref: &str) -> Option<String> {
    let (pkg, name, _) = reg.resolve_type(type_ref, None)?;
    reg.collected_attrs(&pkg, &name)
        .into_iter()
        .find(|(_, adef)| adef.get("long_text").and_then(Value::as_bool) == Some(true))
        .map(|(aname, _)| aname)
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

fn collect_md(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
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

// ---- Public API ----

/// Build the (relative-path, file-content) pairs for every node in
/// the graph. Does not touch the filesystem. Used by `export` and,
/// in the WASM build, by the in-memory bundle path.
pub fn export_docs(graph: &Graph) -> Result<Vec<(String, String)>, AllodError> {
    let reg = graph.registry()?;
    let state = graph.fold()?;

    let mut docs: Vec<(String, String)> = Vec::new();

    // Node files.
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
            serde_yaml::to_string(&Value::Mapping(front)).map_err(|e| AllodError::Other(e.to_string()))?;
        let file_content = format!("---\n{front_text}---\n\n{}\n", body.trim_end_matches('\n'));
        docs.push((format!("{primary}/{id}.md"), file_content));
    }

    // Bundle-level metadata: manifest.
    let state_hash = state.state_hash()?;
    let head = graph.head()?.unwrap_or_default();
    let mut manifest = Mapping::new();
    manifest.insert(s("state_hash"), s(&state_hash));
    manifest.insert(s("head"), s(&head));
    manifest.insert(
        s("declared_losses"),
        Value::Sequence(
            ["history", "classification-envelopes", "edge-envelopes", "documents"]
                .iter()
                .map(|l| s(l))
                .collect(),
        ),
    );
    let manifest_text = serde_yaml::to_string(&Value::Mapping(manifest))
        .map_err(|e| AllodError::Other(e.to_string()))?;
    docs.push((".allod/manifest.yaml".to_string(), manifest_text));

    // Schema projection.
    for (name, doc) in graph.schema_docs()? {
        let schema_text = serde_yaml::to_string(&doc)
            .map_err(|e| AllodError::Other(e.to_string()))?;
        docs.push((format!(".allod/schema/{name}.yaml"), schema_text));
    }

    Ok(docs)
}

/// Export the graph as a markdown bundle directory.
///
/// Returns an `ExportReport` with the count of node files written and
/// the state hash encoded in the bundle manifest. Bundle-directory
/// I/O is `std::fs` — the bundle is not the graph store.
pub fn export(graph: &Graph, out: &Path) -> Result<ExportReport, AllodError> {
    let docs = export_docs(graph)?;

    // Count node files (those not under .allod/).
    let files = docs
        .iter()
        .filter(|(path, _)| !path.starts_with(".allod/"))
        .count();

    // Extract state_hash from the manifest entry.
    let state_hash = docs
        .iter()
        .find(|(path, _)| path == ".allod/manifest.yaml")
        .and_then(|(_, content)| {
            serde_yaml::from_str::<Value>(content).ok()
        })
        .and_then(|v| get_str(&v, "state_hash").map(|s| s.to_string()))
        .unwrap_or_default();

    fs::create_dir_all(out).map_err(|e| AllodError::Other(e.to_string()))?;

    for (rel_path, content) in &docs {
        let dest = out.join(rel_path);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| AllodError::Other(e.to_string()))?;
        }
        fs::write(&dest, content).map_err(|e| AllodError::Other(e.to_string()))?;
    }

    Ok(ExportReport { files, state_hash })
}

/// Re-import a markdown bundle directory.
///
/// Unchanged files produce no admissions. Files whose content has
/// been edited since export re-enter through ordinary admission as
/// update proposals. Returns an `ImportReport` with the admission
/// outcome(s) for any edits, plus files that could not be parsed.
pub fn import(
    graph: &Graph,
    bundle: &Path,
    as_principal: &str,
) -> Result<ImportReport, AllodError> {
    let reg = graph.registry()?;
    graph.fold()?;

    let manifest_path = bundle.join(".allod/manifest.yaml");
    let manifest_text =
        fs::read_to_string(&manifest_path).map_err(|e| AllodError::Other(e.to_string()))?;
    let manifest: Value =
        serde_yaml::from_str(&manifest_text).map_err(|e| AllodError::Other(e.to_string()))?;
    let manifest_hash = get_str(&manifest, "state_hash").unwrap_or("").to_string();

    let mut files = Vec::new();
    collect_md(bundle, &mut files);

    let mut unchanged = 0usize;
    let mut updates: Vec<Value> = Vec::new();
    let mut skipped: Vec<(PathBuf, String)> = Vec::new();

    for path in &files {
        let text = fs::read_to_string(path).map_err(|e| AllodError::Other(e.to_string()))?;
        let (front, body) = match parse_file(&text) {
            Ok(v) => v,
            Err(e) => {
                skipped.push((path.clone(), format!("{}: {e}", path.display())));
                continue;
            }
        };
        let (payload, front_rev, recomputed) = match reconstruct(&reg, &front, &body) {
            Ok(v) => v,
            Err(e) => {
                skipped.push((path.clone(), e));
                continue;
            }
        };
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
        return Ok(ImportReport {
            admissions: vec![],
            unchanged,
            edited_files: 0,
            skipped,
            manifest_hash,
        });
    }

    let kp = graph.load_key(as_principal)?;
    let n = updates.len();
    let (cs, hash) =
        build_changeset(graph, &kp, &format!("Bundle re-ingest: {n} edited file(s)"), updates)?;
    let admission = admit_or_hold(graph, as_principal, &cs, &hash, vec![])?;

    Ok(ImportReport {
        admissions: vec![admission],
        unchanged,
        edited_files: n,
        skipped,
        manifest_hash,
    })
}
