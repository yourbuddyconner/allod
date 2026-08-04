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

/// Process already-parsed `(name, doc)` pairs, building a registry and
/// returning a `Loaded`. This is the core of loading; `load_dir` delegates
/// to it after reading and parsing files from disk.
pub fn load_docs(input: &[(String, Value)]) -> Loaded {
    let mut issues = Vec::new();
    let docs: Vec<(PathBuf, Value)> = input
        .iter()
        .map(|(name, doc)| (PathBuf::from(name), doc.clone()))
        .collect();
    let mut registry = Registry::default();
    for (path, doc) in &docs {
        if doc.get("ontology").is_some() {
            if !registry.register_ontology(doc) {
                issues.push(LoadIssue {
                    path: path.clone(),
                    context: "ontology".into(),
                    message: "missing or invalid package name".into(),
                });
            }
        } else if doc.get("taxonomy").is_some() && !registry.register_taxonomy(doc) {
            issues.push(LoadIssue {
                path: path.clone(),
                context: "taxonomy".into(),
                message: "missing or invalid taxonomy name".into(),
            });
        }
    }
    Loaded {
        registry,
        docs,
        issues,
    }
}

/// Load every .yaml file under `root`, registering ontologies and
/// taxonomies so references resolve regardless of file order.
pub fn load_dir(root: &Path) -> Loaded {
    let mut docs = Vec::new();
    let mut issues = Vec::new();
    let mut files = Vec::new();
    collect_yaml(root, &mut files);
    files.sort();
    for path in files {
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) => {
                issues.push(LoadIssue {
                    path: path.clone(),
                    context: "io".into(),
                    message: err.to_string(),
                });
                continue;
            }
        };
        for de in serde_yaml::Deserializer::from_str(&text) {
            match Value::deserialize(de) {
                Ok(doc) if doc.is_mapping() => docs.push((path.clone(), doc)),
                Ok(_) => {}
                Err(err) => {
                    issues.push(LoadIssue {
                        path: path.clone(),
                        context: "yaml".into(),
                        message: format!("parse failure: {err}"),
                    });
                    break;
                }
            }
        }
    }
    let mut registry = Registry::default();
    for (path, doc) in &docs {
        if doc.get("ontology").is_some() {
            if !registry.register_ontology(doc) {
                issues.push(LoadIssue {
                    path: path.clone(),
                    context: "ontology".into(),
                    message: "missing or invalid package name".into(),
                });
            }
        } else if doc.get("taxonomy").is_some() && !registry.register_taxonomy(doc) {
            issues.push(LoadIssue {
                path: path.clone(),
                context: "taxonomy".into(),
                message: "missing or invalid taxonomy name".into(),
            });
        }
    }
    Loaded {
        registry,
        docs,
        issues,
    }
}
