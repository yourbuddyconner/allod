//! Load projection-form YAML packages from a directory tree.

use crate::registry::Registry;
use serde::Deserialize;
use serde_yaml::Value;
use std::fs;
use std::path::{Path, PathBuf};

/// A problem encountered while loading, before any semantic checks.
pub struct LoadIssue {
    pub path: PathBuf,
    pub context: String,
    pub message: String,
}

/// The result of loading a directory: a populated registry, every
/// mapping document in file order, and any load-time issues.
pub struct Loaded {
    pub registry: Registry,
    pub docs: Vec<(PathBuf, Value)>,
    pub issues: Vec<LoadIssue>,
}

fn collect_yaml(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect_yaml(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("yaml") {
            out.push(path);
        }
    }
}

/// Shared core: given per-file buckets of (path, parsed mapping docs, pre-parse issues),
/// build the registry and collect all issues in file order, interleaving IO/parse issues
/// with registry issues for each file before moving to the next.
fn build_loaded(per_file: Vec<(PathBuf, Vec<Value>, Vec<LoadIssue>)>) -> Loaded {
    let mut all_docs: Vec<(PathBuf, Value)> = Vec::new();
    let mut all_issues: Vec<LoadIssue> = Vec::new();
    let mut registry = Registry::default();

    for (path, file_docs, pre_issues) in per_file {
        // First: emit any IO/parse issues for this file.
        all_issues.extend(pre_issues);
        // Then: register each doc and emit registry issues immediately (interleaved, per-file).
        for doc in file_docs {
            if doc.get("ontology").is_some() {
                if !registry.register_ontology(&doc) {
                    all_issues.push(LoadIssue {
                        path: path.clone(),
                        context: "ontology".into(),
                        message: "missing or invalid package name".into(),
                    });
                }
            } else if doc.get("taxonomy").is_some() && !registry.register_taxonomy(&doc) {
                all_issues.push(LoadIssue {
                    path: path.clone(),
                    context: "taxonomy".into(),
                    message: "missing or invalid taxonomy name".into(),
                });
            }
            all_docs.push((path.clone(), doc));
        }
    }

    Loaded {
        registry,
        docs: all_docs,
        issues: all_issues,
    }
}

/// Process already-parsed `(name, doc)` pairs, building a registry and
/// returning a `Loaded`. This is the core of loading; `load_dir` delegates
/// to it after reading and parsing files from disk.
pub fn load_docs(input: &[(String, Value)]) -> Loaded {
    // Group docs by name (each unique name = one logical "file").
    // Since input is already flat (name, doc) pairs with no IO errors,
    // we produce one per-file bucket per unique name in order.
    let mut seen: Vec<String> = Vec::new();
    let mut buckets: std::collections::HashMap<String, Vec<Value>> =
        std::collections::HashMap::new();
    for (name, doc) in input {
        if !buckets.contains_key(name) {
            seen.push(name.clone());
        }
        buckets.entry(name.clone()).or_default().push(doc.clone());
    }
    let per_file: Vec<(PathBuf, Vec<Value>, Vec<LoadIssue>)> = seen
        .into_iter()
        .map(|name| {
            let docs = buckets.remove(&name).unwrap_or_default();
            (PathBuf::from(&name), docs, vec![])
        })
        .collect();
    build_loaded(per_file)
}

/// Load every .yaml file under `root`, registering ontologies and
/// taxonomies so references resolve regardless of file order.
pub fn load_dir(root: &Path) -> Loaded {
    let mut files = Vec::new();
    collect_yaml(root, &mut files);
    files.sort();

    let mut per_file: Vec<(PathBuf, Vec<Value>, Vec<LoadIssue>)> = Vec::new();

    for path in files {
        let mut file_docs: Vec<Value> = Vec::new();
        let mut pre_issues: Vec<LoadIssue> = Vec::new();

        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) => {
                pre_issues.push(LoadIssue {
                    path: path.clone(),
                    context: "io".into(),
                    message: err.to_string(),
                });
                per_file.push((path, file_docs, pre_issues));
                continue;
            }
        };
        for de in serde_yaml::Deserializer::from_str(&text) {
            match Value::deserialize(de) {
                Ok(doc) if doc.is_mapping() => file_docs.push(doc),
                Ok(_) => {}
                Err(err) => {
                    pre_issues.push(LoadIssue {
                        path: path.clone(),
                        context: "yaml".into(),
                        message: format!("parse failure: {err}"),
                    });
                    break;
                }
            }
        }
        per_file.push((path, file_docs, pre_issues));
    }

    build_loaded(per_file)
}
