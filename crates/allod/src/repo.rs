//! CLI shim: thin delegation to allod_graph::repo.
//! Opens the graph from disk, delegates, then prints.

use allod_core::store::Graph;
use allod_graph::repo as lib;
use std::path::Path;

pub use lib::SCAN_TOOL;

/// Import one commit as one derived changeset. Returns (hash, admitted).
pub fn import_commit(
    dir: &Path,
    repo: &Path,
    commit: &str,
    indexer: &str,
) -> Result<(String, bool), String> {
    let graph = Graph::open(dir)?;
    lib::import_commit(&graph, repo, commit, indexer).map_err(|e| e.to_string())
}

/// Render the semantic diff to stdout (and optionally write the artifact).
/// Byte-identical to the original output.
pub fn semantic_diff(dir: &Path, cs_hash: &str, out: Option<&Path>) -> Result<(), String> {
    let graph = Graph::open(dir)?;
    let diff = lib::semantic_diff(&graph, cs_hash).map_err(|e| e.to_string())?;

    println!("  semantic diff of {}:", crate::short(cs_hash));
    for e in &diff.entries {
        let mut text = format!(
            "{}d function {}; {} inbound caller(s): {}",
            e.verb,
            e.name,
            e.callers.len(),
            if e.callers.is_empty() { "none".into() } else { e.callers.join(", ") },
        );
        if !e.classified.is_empty() {
            text.push_str(&format!("; classified {}", e.classified.join(", ")));
        }
        println!("    · {text}");
    }
    if diff.entries.is_empty() {
        println!("    · no function-level changes in this changeset");
    }
    if let Some(out) = out {
        std::fs::write(
            out,
            serde_yaml::to_string(&diff.artifact).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        println!("  ✓ review artifact written to {}", out.display());
    }
    Ok(())
}

/// Create the demo repository: two commits, a spend path, and a
/// second commit that touches it and adds a caller.
pub fn make_sample_repo(dir: &Path) -> Result<(String, String), String> {
    lib::make_sample_repo(dir).map_err(|e| e.to_string())
}
