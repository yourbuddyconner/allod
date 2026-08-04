//! CLI shim for the markdown bundle binding (§7.2).
//!
//! Opens the graph from disk and delegates to `allod_graph::md`, then
//! prints the byte-identical output that the mvp acceptance test expects.

use allod_core::store::Graph;
use allod_graph::ops::{short, Admission};
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

    // Read the manifest hash before delegating (for the round-trip message).
    let manifest_text =
        std::fs::read_to_string(bundle.join(".allod/manifest.yaml")).map_err(|e| e.to_string())?;
    let manifest: serde_yaml::Value =
        serde_yaml::from_str(&manifest_text).map_err(|e| e.to_string())?;
    let manifest_hash = allod_core::get_str(&manifest, "state_hash")
        .unwrap_or("")
        .to_string();

    let report = allod_graph::md::import(&graph, bundle, as_principal).map_err(|e| e.to_string())?;

    if report.admissions.is_empty() && report.skipped.is_empty() {
        // Round-trip path.
        let state = graph.fold()?;
        let current = state.state_hash()?;
        if current == manifest_hash {
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
                short(&manifest_hash),
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
            print_admission(admission);
        }
    }
    Ok(())
}

fn print_admission(admission: &Admission) {
    match admission {
        Admission::Admitted { hash, matched_rules } => {
            let basis = if matched_rules.is_empty() {
                "root authority, default posture".to_string()
            } else {
                format!("rules: {}", matched_rules.join(", "))
            };
            println!("  ✓ admitted {} ({basis})", short(hash));
        }
        Admission::Held { hash, checklist } => {
            println!("  ⧗ held as proposal {}", short(hash));
            println!(
                "      matched rules: {}",
                checklist.matched_rules.join(", ")
            );
            for (role, quorum) in &checklist.reviewers {
                println!("      requires: reviewers role {role} (quorum {quorum})");
            }
            for class in &checklist.attestations {
                println!("      requires: attestation from class {class}");
            }
            if checklist.root_required {
                println!("      requires: root authority (default posture)");
            }
        }
    }
}
