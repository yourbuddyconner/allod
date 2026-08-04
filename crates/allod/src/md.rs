//! CLI shim for the markdown bundle binding (§7.2).
//!
//! Opens the graph from disk and delegates to `allod_graph::md`, then
//! prints the byte-identical output that the mvp acceptance test expects.

use allod_core::store::Graph;
use allod_graph::ops::short;
use std::path::Path;

pub fn export(graph_dir: &Path, out: &Path) -> Result<(), String> {
    let graph = Graph::open(graph_dir)?;
    let report = allod_graph::md::export(&graph, out).map_err(|e| e.to_string())?;
    println!(
        "  ✓ exported {} nodes (state {})",
        report.files,
        short(&report.state_hash)
    );
    Ok(())
}

pub fn import(graph_dir: &Path, bundle: &Path, as_principal: &str) -> Result<(), String> {
    let graph = Graph::open(graph_dir)?;

    let report = allod_graph::md::import(&graph, bundle, as_principal).map_err(|e| e.to_string())?;

    // Abort if any files were malformed (restoring old CLI behaviour).
    if let Some((path, reason)) = report.skipped.first() {
        return Err(format!("malformed file {}: {reason}", path.display()));
    }

    if report.admissions.is_empty() {
        // Round-trip path.
        let state = graph.fold()?;
        let current = state.state_hash()?;
        if current == report.manifest_hash {
            println!(
                "  ✓ round trip verified: {} nodes unchanged, state {} matches \
                 the bundle manifest (§7.4)",
                report.unchanged,
                short(&current)
            );
        } else {
            println!(
                "  ⚠ bundle is unmodified but the graph has moved on: bundle state {}, \
                 graph state {}",
                short(&report.manifest_hash),
                short(&current)
            );
        }
    } else {
        // Edit re-ingest path: "  {unchanged} unchanged, {n} edited — …"
        let n = report.edited_files;
        println!(
            "  {} unchanged, {n} edited — the edit enters admission (§7.2):",
            report.unchanged
        );
        for admission in &report.admissions {
            crate::print_admission(admission);
        }
    }
    Ok(())
}
