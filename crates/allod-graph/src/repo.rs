//! Repo import (§8.3): derive a knowledge graph about code from a
//! git repository, commit-aligned, and compute the semantic diff.
//!
//! The extractor is tree-sitter-grade syntactic scanning for Rust —
//! deterministic, at lower resolution, which §8.3 blesses as a
//! conforming fallback. Its identity travels in lineage as
//! `allod-scan@0.1`, and call edges are name-matched, so resolution
//! limits are the tool's, declared, not hidden. Git stays
//! authoritative for content: nodes reference blobs by `git:` ref
//! and the graph never contains source text.
//!
//! Everything in this module requires the `native` cargo feature
//! (spawns git via std::process::Command and touches the filesystem).

use allod_core::fold::State;
use allod_core::get_str;
use allod_core::store::Graph;
use serde_yaml::{Mapping, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use crate::ops::{build_changeset, uuid4, Admission};
use crate::AllodError;

pub const SCAN_TOOL: &str = "allod-scan@0.1";

fn git(repo: &Path, args: &[&str]) -> Result<String, AllodError> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|e| AllodError::Other(format!("git: {e}")))?;
    if !out.status.success() {
        return Err(AllodError::Other(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

// ---------------- deterministic extraction ----------------

#[derive(Clone, Debug)]
struct ExtractedItem {
    kind: String, // "fn" | "struct" | "enum" | "trait"
    name: String,
    signature: String,
    start: usize,
    end: usize,
}

fn item_of_line(trimmed: &str) -> Option<(String, String)> {
    for (prefix, kind) in [
        ("pub fn ", "fn"),
        ("pub(crate) fn ", "fn"),
        ("fn ", "fn"),
        ("pub struct ", "struct"),
        ("struct ", "struct"),
        ("pub enum ", "enum"),
        ("enum ", "enum"),
        ("pub trait ", "trait"),
        ("trait ", "trait"),
    ] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                return Some((kind.to_string(), name));
            }
        }
    }
    None
}

/// Find the item's extent by brace counting from its declaration.
fn item_end(lines: &[&str], start: usize) -> usize {
    let mut depth = 0i64;
    let mut opened = false;
    for (offset, line) in lines[start..].iter().enumerate() {
        for ch in line.chars() {
            match ch {
                '{' => {
                    depth += 1;
                    opened = true;
                }
                '}' => depth -= 1,
                ';' if !opened && depth == 0 => return start + offset,
                _ => {}
            }
        }
        if opened && depth <= 0 {
            return start + offset;
        }
    }
    lines.len().saturating_sub(1)
}

fn extract_items(text: &str) -> Vec<ExtractedItem> {
    let lines: Vec<&str> = text.lines().collect();
    let mut items = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        // Top-level and impl-level items: indentation of 0 or 4.
        let indent = line.len() - trimmed.len();
        if indent > 4 {
            continue;
        }
        if let Some((kind, name)) = item_of_line(trimmed) {
            let end = item_end(&lines, i);
            items.push(ExtractedItem {
                kind,
                name,
                signature: trimmed.trim_end_matches('{').trim().to_string(),
                start: i + 1,
                end: end + 1,
            });
        }
    }
    items
}

/// Name-matched call edges within a function body — the declared
/// resolution limit of a syntactic scanner.
fn find_calls(text: &str, item: &ExtractedItem, fn_names: &BTreeSet<String>) -> BTreeSet<String> {
    let lines: Vec<&str> = text.lines().collect();
    let body = lines[item.start.min(lines.len())..item.end.min(lines.len())].join("\n");
    let mut calls = BTreeSet::new();
    for name in fn_names {
        if name == &item.name {
            continue;
        }
        if body.contains(&format!("{name}(")) {
            calls.insert(name.clone());
        }
    }
    calls
}

// ---------------- language map ----------------

fn language_of(path: &str) -> Option<&'static str> {
    let ext = path.rsplit('.').next()?;
    match ext {
        "rs" => Some("rust"),
        "ts" | "tsx" => Some("typescript"),
        "js" | "jsx" => Some("javascript"),
        "py" => Some("python"),
        "toml" => Some("toml"),
        "yaml" | "yml" => Some("yaml"),
        "md" => Some("markdown"),
        "sh" => Some("shell"),
        "json" => Some("json"),
        _ => None,
    }
}

// ---------------- the derived changeset ----------------

struct ExistingIndex {
    repo_id: Option<String>,
    /// path -> (node id, rev, blob)
    files: BTreeMap<String, (String, String, String)>,
    /// path -> (edge id, rev) for the in_repo edge
    in_repo_edges: BTreeMap<String, (String, String)>,
    /// (path, kind, name) -> (node id, rev, content)
    items: BTreeMap<(String, String, String), (String, String, Value)>,
    /// (path, kind, name) -> (edge id, rev) for the declares edge
    declares_edges: BTreeMap<(String, String, String), (String, String)>,
    /// (from node id, to node id) -> (edge id, rev)
    calls: BTreeMap<(String, String), (String, String)>,
}

fn index_state(state: &State) -> ExistingIndex {
    let mut idx = ExistingIndex {
        repo_id: None,
        files: BTreeMap::new(),
        in_repo_edges: BTreeMap::new(),
        items: BTreeMap::new(),
        declares_edges: BTreeMap::new(),
        calls: BTreeMap::new(),
    };
    let mut file_of_item: BTreeMap<String, String> = BTreeMap::new(); // item id -> path
    let mut file_paths: BTreeMap<String, String> = BTreeMap::new(); // file id -> path
    for ((kind, id), obj) in &state.objects {
        if obj.deleted {
            continue;
        }
        let tref = get_str(&obj.content, "type").map(allod_core::bare).unwrap_or("");
        if kind == "node" {
            match tref {
                "code/Repository" => idx.repo_id = Some(id.clone()),
                "code/SourceFile" => {
                    let attrs = obj.content.get("attributes");
                    let path = attrs.and_then(|a| get_str(a, "path")).unwrap_or("");
                    let blob = attrs.and_then(|a| get_str(a, "blob")).unwrap_or("");
                    file_paths.insert(id.clone(), path.to_string());
                    idx.files.insert(
                        path.to_string(),
                        (id.clone(), obj.rev.clone(), blob.to_string()),
                    );
                }
                _ => {}
            }
        }
    }
    for ((kind, edge_id), obj) in &state.objects {
        if kind != "edge" || obj.deleted {
            continue;
        }
        let tref = get_str(&obj.content, "type").map(allod_core::bare).unwrap_or("");
        match tref {
            "code/declares" => {
                let from = get_str(&obj.content, "from").unwrap_or("");
                let to = get_str(&obj.content, "to").unwrap_or("");
                if let (Some(file_id), Some(item_id)) =
                    (from.strip_prefix("node:"), to.strip_prefix("node:"))
                {
                    if let Some(path) = file_paths.get(file_id) {
                        file_of_item.insert(item_id.to_string(), path.clone());
                        // We'll fill declares_edges after we know item keys (done below)
                        // For now, store a temporary association: edge_id by (file_id, item_id)
                        // We'll process this after building items.
                    }
                }
            }
            "code/in_repo" => {
                let from = get_str(&obj.content, "from").unwrap_or("");
                if let Some(file_id) = from.strip_prefix("node:") {
                    if let Some(path) = file_paths.get(file_id) {
                        idx.in_repo_edges.insert(
                            path.clone(),
                            (edge_id.clone(), obj.rev.clone()),
                        );
                    }
                }
            }
            _ => {}
        }
    }
    // Third pass: items, declares edges, and calls
    // Build item key -> item_id mapping first, then correlate declares edges
    let mut item_key_of_id: BTreeMap<String, (String, String, String)> = BTreeMap::new(); // item id -> key
    for ((kind, id), obj) in &state.objects {
        if obj.deleted {
            continue;
        }
        let tref = get_str(&obj.content, "type").map(allod_core::bare).unwrap_or("");
        if kind == "node" && (tref == "code/Function" || tref == "code/Type") {
            let name = obj
                .content
                .get("attributes")
                .and_then(|a| get_str(a, "name"))
                .unwrap_or("");
            let path = file_of_item.get(id).cloned().unwrap_or_default();
            let item_kind = if tref == "code/Function" {
                "fn".to_string()
            } else {
                obj.content
                    .get("attributes")
                    .and_then(|a| get_str(a, "kind"))
                    .unwrap_or("struct")
                    .to_string()
            };
            let key = (path, item_kind, name.to_string());
            item_key_of_id.insert(id.clone(), key.clone());
            idx.items.insert(
                key,
                (id.clone(), obj.rev.clone(), obj.content.clone()),
            );
        } else if kind == "edge" && tref == "code/calls" {
            let from = get_str(&obj.content, "from").unwrap_or("").to_string();
            let to = get_str(&obj.content, "to").unwrap_or("").to_string();
            idx.calls.insert((from, to), (id.clone(), obj.rev.clone()));
        }
    }
    // Fourth pass: now that item_key_of_id is built, fill declares_edges
    for ((kind, edge_id), obj) in &state.objects {
        if kind != "edge" || obj.deleted {
            continue;
        }
        let tref = get_str(&obj.content, "type").map(allod_core::bare).unwrap_or("");
        if tref == "code/declares" {
            let to = get_str(&obj.content, "to").unwrap_or("");
            if let Some(item_id) = to.strip_prefix("node:") {
                if let Some(key) = item_key_of_id.get(item_id) {
                    idx.declares_edges.insert(
                        key.clone(),
                        (edge_id.clone(), obj.rev.clone()),
                    );
                }
            }
        }
    }
    idx
}

fn s(v: &str) -> Value {
    Value::String(v.to_string())
}

fn provenance(indexer: &str, commit: &str) -> Value {
    let mut prov = Mapping::new();
    prov.insert(
        s("derived_from"),
        Value::Sequence(vec![s(&format!("git:#{commit}"))]),
    );
    prov.insert(s("derived_by"), s(&format!("principal:{indexer}")));
    prov.insert(s("method"), s("deterministic"));
    prov.insert(s("tool"), s(SCAN_TOOL));
    Value::Mapping(prov)
}

fn node_op(verb: &str, payload: Mapping) -> Value {
    let mut op = Mapping::new();
    op.insert(s(verb), Value::Mapping(payload));
    Value::Mapping(op)
}

/// Content comparison ignores provenance: an unchanged entity must
/// not update just because the deriving commit moved (§8.4).
fn content_differs(existing: &Value, desired_attrs: &Mapping) -> bool {
    existing.get("attributes").and_then(Value::as_mapping) != Some(desired_attrs)
}

/// Import one commit as one derived changeset (§8.3). Returns the
/// changeset hash and whether it was admitted outright.
pub fn import_commit(
    graph: &Graph,
    repo: &Path,
    commit: &str,
    indexer: &str,
) -> Result<(String, bool), AllodError> {
    let kp = graph.load_key(indexer)?;
    let commit = git(repo, &["rev-parse", commit])?.trim().to_string();
    let state = graph.fold()?;
    let idx = index_state(&state);
    let origin = repo
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo")
        .to_string();

    // What the repo looks like at this commit.
    let mut desired_files: BTreeMap<String, String> = BTreeMap::new(); // path -> blob sha
    for line in git(repo, &["ls-tree", "-r", &commit])?.lines() {
        let Some((meta, path)) = line.split_once('\t') else { continue };
        if path.starts_with(".allod/") {
            continue;
        }
        let parts: Vec<&str> = meta.split_whitespace().collect();
        if parts.len() == 3 && parts[1] == "blob" {
            desired_files.insert(path.to_string(), parts[2].to_string());
        }
    }
    // Only fetch source text for Rust files (item extraction).
    let mut sources: BTreeMap<String, String> = BTreeMap::new();
    for path in desired_files.keys() {
        if path.ends_with(".rs") {
            sources.insert(
                path.clone(),
                git(repo, &["show", &format!("{commit}:{path}")])?,
            );
        }
    }
    let mut desired_items: BTreeMap<(String, String, String), ExtractedItem> = BTreeMap::new();
    let mut fn_names: BTreeSet<String> = BTreeSet::new();
    for (path, text) in &sources {
        for item in extract_items(text) {
            if item.kind == "fn" {
                fn_names.insert(item.name.clone());
            }
            desired_items.insert((path.clone(), item.kind.clone(), item.name.clone()), item);
        }
    }
    // Name-resolved calls: (caller key) -> callee keys.
    let mut desired_calls: BTreeSet<((String, String, String), (String, String, String))> =
        BTreeSet::new();
    let callee_of = |name: &str| -> Option<(String, String, String)> {
        desired_items
            .keys()
            .find(|(_, kind, n)| kind == "fn" && n == name)
            .cloned()
    };
    for (key, item) in &desired_items {
        if item.kind != "fn" {
            continue;
        }
        for callee in find_calls(&sources[&key.0], item, &fn_names) {
            if let Some(callee_key) = callee_of(&callee) {
                desired_calls.insert((key.clone(), callee_key));
            }
        }
    }

    // Diff against the existing derived graph.
    let mut ops: Vec<Value> = Vec::new();
    let prov = provenance(indexer, &commit);
    let repo_id = match &idx.repo_id {
        Some(id) => id.clone(),
        None => {
            let id = uuid4();
            let mut attrs = Mapping::new();
            attrs.insert(s("name"), s(&origin));
            attrs.insert(s("origin"), s(&repo.display().to_string()));
            let mut node = Mapping::new();
            node.insert(s("kind"), s("node"));
            node.insert(s("id"), s(&id));
            node.insert(s("type"), s("code/Repository@1"));
            node.insert(s("attributes"), Value::Mapping(attrs));
            node.insert(s("provenance"), prov.clone());
            ops.push(node_op("create", node));
            id
        }
    };

    let mut file_ids: BTreeMap<String, String> = BTreeMap::new();
    for (path, blob) in &desired_files {
        let blob_ref = format!("git:{origin}#{blob}:{path}");
        let mut attrs = Mapping::new();
        attrs.insert(s("path"), s(path));
        if let Some(lang) = language_of(path) {
            attrs.insert(s("language"), s(lang));
        }
        attrs.insert(s("blob"), s(&blob_ref));
        match idx.files.get(path) {
            Some((id, rev, old_blob)) => {
                file_ids.insert(path.clone(), id.clone());
                if old_blob != &blob_ref {
                    let mut node = Mapping::new();
                    node.insert(s("kind"), s("node"));
                    node.insert(s("id"), s(id));
                    node.insert(s("prior"), s(rev));
                    node.insert(s("type"), s("code/SourceFile@1"));
                    node.insert(s("attributes"), Value::Mapping(attrs));
                    node.insert(s("provenance"), prov.clone());
                    ops.push(node_op("update", node));
                }
            }
            None => {
                let id = uuid4();
                file_ids.insert(path.clone(), id.clone());
                let mut node = Mapping::new();
                node.insert(s("kind"), s("node"));
                node.insert(s("id"), s(&id));
                node.insert(s("type"), s("code/SourceFile@1"));
                node.insert(s("attributes"), Value::Mapping(attrs));
                node.insert(s("provenance"), prov.clone());
                ops.push(node_op("create", node));
                let mut edge = Mapping::new();
                edge.insert(s("kind"), s("edge"));
                edge.insert(s("id"), s(&uuid4()));
                edge.insert(s("type"), s("code/in_repo@1"));
                edge.insert(s("from"), s(&format!("node:{id}")));
                edge.insert(s("to"), s(&format!("node:{repo_id}")));
                edge.insert(s("provenance"), prov.clone());
                ops.push(node_op("create", edge));
            }
        }
    }

    let mut item_ids: BTreeMap<(String, String, String), String> = BTreeMap::new();
    for (key, item) in &desired_items {
        let (path, kind, name) = key;
        let (type_ref, attrs) = if kind == "fn" {
            let mut attrs = Mapping::new();
            attrs.insert(s("name"), s(name));
            attrs.insert(s("signature"), s(&item.signature));
            attrs.insert(s("span"), s(&format!("L{}-L{}", item.start, item.end)));
            ("code/Function@1", attrs)
        } else {
            let mut attrs = Mapping::new();
            attrs.insert(s("name"), s(name));
            attrs.insert(
                s("kind"),
                s(if kind == "trait" { "interface" } else { kind }),
            );
            ("code/Type@1", attrs)
        };
        match idx.items.get(key) {
            Some((id, rev, existing)) => {
                item_ids.insert(key.clone(), id.clone());
                if content_differs(existing, &attrs) {
                    let mut node = Mapping::new();
                    node.insert(s("kind"), s("node"));
                    node.insert(s("id"), s(id));
                    node.insert(s("prior"), s(rev));
                    node.insert(s("type"), s(type_ref));
                    node.insert(s("attributes"), Value::Mapping(attrs));
                    node.insert(s("provenance"), prov.clone());
                    ops.push(node_op("update", node));
                }
            }
            None => {
                let id = uuid4();
                item_ids.insert(key.clone(), id.clone());
                let mut node = Mapping::new();
                node.insert(s("kind"), s("node"));
                node.insert(s("id"), s(&id));
                node.insert(s("type"), s(type_ref));
                node.insert(s("attributes"), Value::Mapping(attrs));
                node.insert(s("provenance"), prov.clone());
                ops.push(node_op("create", node));
                let file_id = file_ids.get(path).cloned().unwrap_or_default();
                let mut edge = Mapping::new();
                edge.insert(s("kind"), s("edge"));
                edge.insert(s("id"), s(&uuid4()));
                edge.insert(s("type"), s("code/declares@1"));
                edge.insert(s("from"), s(&format!("node:{file_id}")));
                edge.insert(s("to"), s(&format!("node:{id}")));
                edge.insert(s("provenance"), prov.clone());
                ops.push(node_op("create", edge));
            }
        }
    }

    // Call edges: create the new, delete the gone.
    let desired_call_ids: BTreeSet<(String, String)> = desired_calls
        .iter()
        .filter_map(|(from, to)| {
            Some((
                format!("node:{}", item_ids.get(from)?),
                format!("node:{}", item_ids.get(to)?),
            ))
        })
        .collect();
    for (from, to) in &desired_call_ids {
        if !idx.calls.contains_key(&(from.clone(), to.clone())) {
            let mut edge = Mapping::new();
            edge.insert(s("kind"), s("edge"));
            edge.insert(s("id"), s(&uuid4()));
            edge.insert(s("type"), s("code/calls@1"));
            edge.insert(s("from"), s(from));
            edge.insert(s("to"), s(to));
            edge.insert(s("provenance"), prov.clone());
            ops.push(node_op("create", edge));
        }
    }
    for ((from, to), (edge_id, rev)) in &idx.calls {
        if !desired_call_ids.contains(&(from.clone(), to.clone())) {
            let mut del = Mapping::new();
            del.insert(s("kind"), s("edge"));
            del.insert(s("id"), s(edge_id));
            del.insert(s("prior"), s(rev));
            ops.push(node_op("delete", del));
        }
    }
    // Items that disappeared: delete declares edge + item node (+ any calls edges
    // touching them, which were already handled in the calls loop above).
    for (key, (item_id, item_rev, _)) in &idx.items {
        if !desired_items.contains_key(key) {
            // Delete the declares edge first (fold rejects dangling edges).
            if let Some((edge_id, edge_rev)) = idx.declares_edges.get(key) {
                let mut del = Mapping::new();
                del.insert(s("kind"), s("edge"));
                del.insert(s("id"), s(edge_id));
                del.insert(s("prior"), s(edge_rev));
                ops.push(node_op("delete", del));
            }
            // Delete the item node.
            let mut del = Mapping::new();
            del.insert(s("kind"), s("node"));
            del.insert(s("id"), s(item_id));
            del.insert(s("prior"), s(item_rev));
            ops.push(node_op("delete", del));
        }
    }

    // Files that disappeared: delete in_repo edge + SourceFile node.
    // (Items in disappeared files are already handled above since desired_items
    // won't contain their keys either.)
    for (path, (file_id, file_rev, _)) in &idx.files {
        if !desired_files.contains_key(path) {
            // Delete the in_repo edge first.
            if let Some((edge_id, edge_rev)) = idx.in_repo_edges.get(path) {
                let mut del = Mapping::new();
                del.insert(s("kind"), s("edge"));
                del.insert(s("id"), s(edge_id));
                del.insert(s("prior"), s(edge_rev));
                ops.push(node_op("delete", del));
            }
            // Delete the SourceFile node.
            let mut del = Mapping::new();
            del.insert(s("kind"), s("node"));
            del.insert(s("id"), s(file_id));
            del.insert(s("prior"), s(file_rev));
            ops.push(node_op("delete", del));
        }
    }

    if ops.is_empty() {
        return Err(AllodError::Other("nothing changed at this commit".into()));
    }
    let n = ops.len();
    let (cs, hash) = build_changeset(
        graph,
        &kp,
        &format!("Derived from commit {} ({n} ops, {SCAN_TOOL})", &commit[..12.min(commit.len())]),
        ops,
    )?;
    let admission = crate::ops::admit_or_hold(graph, indexer, &cs, &hash, vec![])?;
    let admitted = matches!(admission, Admission::Admitted { .. });
    Ok((hash, admitted))
}

// ---------------- typed semantic diff result ----------------

/// One function entry in a semantic diff.
pub struct DiffEntry {
    /// The function's node ref (`node:<id>`).
    pub node_ref: String,
    /// The function's name.
    pub name: String,
    /// The changeset verb: "create" or "update".
    pub verb: String,
    /// Inbound callers (by name) from the current state plus
    /// the edges this changeset itself introduces.
    pub callers: Vec<String>,
    /// Governance classifications applied to this node.
    pub classified: Vec<String>,
}

/// The result of `semantic_diff`: the changed-function structure,
/// ready for the caller to render or inspect.
pub struct SemanticDiff {
    /// The changeset hash (short form: caller must shorten if desired).
    pub cs_hash: String,
    /// Per-function entries — only function nodes touched by the changeset.
    pub entries: Vec<DiffEntry>,
    /// The review artifact YAML document, ready to write to disk.
    pub artifact: Value,
}

/// Semantic diff (§8.3): read a derived changeset and compute the
/// knowledge-level diff with the inbound-call blast radius.
///
/// Returns a typed [`SemanticDiff`]; the caller decides how to render it.
/// This function never prints.
pub fn semantic_diff(graph: &Graph, cs_hash: &str) -> Result<SemanticDiff, AllodError> {
    let state = graph.fold()?;
    let cs = graph
        .read_changeset(cs_hash)
        .or_else(|_| graph.read_proposal(cs_hash))?;
    let ops = cs
        .get("operations")
        .and_then(Value::as_sequence)
        .ok_or_else(|| AllodError::Other("changeset has no operations".into()))?;

    // Names and call edges introduced by this changeset itself.
    let mut cs_names: BTreeMap<String, String> = BTreeMap::new(); // node ref -> fn name
    let mut cs_calls: Vec<(String, String)> = Vec::new(); // (from ref, to ref)
    for op in ops {
        let Some((_, payload)) = op.as_mapping().and_then(|m| m.iter().next()) else {
            continue;
        };
        let tref = get_str(payload, "type").map(allod_core::bare);
        if get_str(payload, "kind") == Some("node") && tref == Some("code/Function") {
            if let (Some(id), Some(name)) = (
                get_str(payload, "id"),
                payload.get("attributes").and_then(|a| get_str(a, "name")),
            ) {
                cs_names.insert(format!("node:{id}"), name.to_string());
            }
        } else if get_str(payload, "kind") == Some("edge") && tref == Some("code/calls") {
            if let (Some(from), Some(to)) = (get_str(payload, "from"), get_str(payload, "to")) {
                cs_calls.push((from.to_string(), to.to_string()));
            }
        }
    }
    let name_of = |reference: &str| -> Option<String> {
        cs_names.get(reference).cloned().or_else(|| {
            let (_, obj) = state.resolve_ref(reference)?;
            obj.content
                .get("attributes")
                .and_then(|a| get_str(a, "name"))
                .map(String::from)
        })
    };

    let mut entries = Vec::new();
    for op in ops {
        let Some((verb, payload)) = op.as_mapping().and_then(|m| m.iter().next()) else {
            continue;
        };
        let verb_str = verb.as_str().unwrap_or("");
        if get_str(payload, "kind") != Some("node")
            || get_str(payload, "type").map(allod_core::bare) != Some("code/Function")
        {
            continue;
        }
        let id = get_str(payload, "id").unwrap_or("");
        let name = payload
            .get("attributes")
            .and_then(|a| get_str(a, "name"))
            .unwrap_or("?");
        let node_ref = format!("node:{id}");
        // Blast radius: inbound calls from the current state plus
        // the edges this changeset itself introduces.
        let mut callers: BTreeSet<String> = state
            .objects
            .iter()
            .filter(|((k, _), o)| {
                k == "edge"
                    && !o.deleted
                    && get_str(&o.content, "type").map(allod_core::bare)
                        == Some("code/calls")
                    && get_str(&o.content, "to") == Some(node_ref.as_str())
            })
            .filter_map(|(_, o)| name_of(get_str(&o.content, "from")?))
            .collect();
        callers.extend(
            cs_calls
                .iter()
                .filter(|(_, to)| to == &node_ref)
                .filter_map(|(from, _)| name_of(from)),
        );
        let callers: Vec<String> = callers.into_iter().collect();
        let classified: Vec<String> = state.classifications_of(&node_ref);
        entries.push(DiffEntry {
            node_ref: node_ref.clone(),
            name: name.to_string(),
            verb: verb_str.to_string(),
            callers,
            classified,
        });
    }

    // Build the review artifact.
    let sections: Vec<Value> = entries
        .iter()
        .map(|e| {
            let text = format!(
                "{}d function {}; {} inbound caller(s): {}{}",
                e.verb,
                e.name,
                e.callers.len(),
                if e.callers.is_empty() {
                    "none".into()
                } else {
                    e.callers.join(", ")
                },
                if e.classified.is_empty() {
                    String::new()
                } else {
                    format!("; classified {}", e.classified.join(", "))
                },
            );
            let mut section = Mapping::new();
            section.insert(s("anchor"), s(&e.node_ref));
            section.insert(s("heading"), s(&e.name));
            section.insert(s("text"), s(&text));
            Value::Mapping(section)
        })
        .collect();
    let mut artifact = Mapping::new();
    artifact.insert(s("kind"), s("review-artifact"));
    artifact.insert(s("subject"), s(cs_hash));
    artifact.insert(s("sections"), Value::Sequence(sections));
    artifact.insert(s("verdict"), s("open"));

    Ok(SemanticDiff {
        cs_hash: cs_hash.to_string(),
        entries,
        artifact: Value::Mapping(artifact),
    })
}

/// Create the demo repository: two commits, a spend path, and a
/// second commit that touches it and adds a caller.
pub fn make_sample_repo(dir: &Path) -> Result<(String, String), AllodError> {
    std::fs::create_dir_all(dir.join("src")).map_err(|e| AllodError::Other(e.to_string()))?;
    git(dir, &["init", "-q"])?;
    let commit = |msg: &str| -> Result<String, AllodError> {
        git(dir, &["add", "-A"])?;
        git(
            dir,
            &[
                "-c", "user.name=allod-demo", "-c", "user.email=demo@allod.example",
                "commit", "-q", "-m", msg,
            ],
        )?;
        Ok(git(dir, &["rev-parse", "HEAD"])?.trim().to_string())
    };
    std::fs::write(
        dir.join("src/lib.rs"),
        "pub fn authorize_spend(amount: u64) -> bool {\n    validate(amount)\n}\n\n\
         fn validate(amount: u64) -> bool {\n    amount < 10_000\n}\n\n\
         pub struct SpendRequest {\n    pub amount: u64,\n}\n",
    )
    .map_err(|e| AllodError::Other(e.to_string()))?;
    let first = commit("spend path")?;
    std::fs::write(
        dir.join("src/lib.rs"),
        "pub fn authorize_spend(amount: u64) -> bool {\n    audit(amount);\n    \
         validate(amount) && amount > 0\n}\n\n\
         fn validate(amount: u64) -> bool {\n    amount < 10_000\n}\n\n\
         fn audit(amount: u64) {\n    let _ = amount;\n}\n\n\
         pub fn refund(amount: u64) -> bool {\n    authorize_spend(amount)\n}\n\n\
         pub struct SpendRequest {\n    pub amount: u64,\n}\n",
    )
    .map_err(|e| AllodError::Other(e.to_string()))?;
    let second = commit("audit the spend path, add refunds")?;
    Ok((first, second))
}
