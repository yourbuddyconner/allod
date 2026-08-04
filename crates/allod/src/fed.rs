//! CLI shim for federation (Part 9).
//!
//! Opens the graph from disk and delegates to `allod_graph::fed`; all
//! file I/O and stdout printing live here. The library (`allod_graph::fed`)
//! is pure: no filesystem access, no printing.

use allod_core::store::Graph;
use allod_graph::ops::short;
use serde_yaml::Value;
use std::fs;
use std::path::Path;

pub fn peer_add(
    dir: &Path,
    name: &str,
    graph_id: &str,
    root_key_hex: &str,
    by: &str,
) -> Result<(), String> {
    let graph = Graph::open(dir)?;
    allod_graph::fed::peer_add(&graph, name, graph_id, root_key_hex, by)
        .map_err(|e| e.to_string())
}

pub fn grant(dir: &Path, audience: &str, region: &str, by: &str) -> Result<String, String> {
    let graph = Graph::open(dir)?;
    allod_graph::fed::grant(&graph, audience, region, by).map_err(|e| e.to_string())
}

pub fn revoke(dir: &Path, grant_id: &str, by: &str) -> Result<(), String> {
    let graph = Graph::open(dir)?;
    allod_graph::fed::revoke(&graph, grant_id, by).map_err(|e| e.to_string())
}

/// Produce a share bundle and write it to `out`. Prints the same line
/// the old monolithic implementation printed.
pub fn bundle(dir: &Path, grant_id: &str, out: &Path, by: &str) -> Result<(), String> {
    let graph = Graph::open(dir)?;
    let doc = allod_graph::fed::make_bundle(&graph, grant_id, by)
        .map_err(|e| e.to_string())?;

    // Count the disclosed objects for the output line.
    let obj_count = doc
        .get("objects")
        .and_then(Value::as_sequence)
        .map(|s| s.len())
        .unwrap_or(0);
    let region = doc
        .get("grant")
        .and_then(|g| g.get("scope"))
        .and_then(|sc| allod_core::get_str(sc, "region"))
        .unwrap_or("")
        .to_string();
    let checkpoint_rev = doc
        .get("checkpoint")
        .and_then(|cp| allod_core::get_str(cp, "revision"))
        .unwrap_or("")
        .to_string();
    let state_hash = doc
        .get("checkpoint")
        .and_then(|cp| allod_core::get_str(cp, "state_hash"))
        .unwrap_or("")
        .to_string();

    fs::write(
        out,
        serde_yaml::to_string(&doc).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;

    println!(
        "  ✓ bundle: {} objects in region {region}, checkpoint {} (state {})",
        obj_count,
        short(&checkpoint_rev),
        short(&state_hash)
    );
    Ok(())
}

/// Verify the bundle at `bundle_path` and optionally import one object.
/// Prints the byte-identical output the old implementation produced.
pub fn import(
    dir: &Path,
    bundle_path: &Path,
    by: &str,
    import_id: Option<&str>,
) -> Result<Option<String>, String> {
    let graph = Graph::open(dir)?;
    let bundle: Value = serde_yaml::from_str(
        &fs::read_to_string(bundle_path).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;

    // Print the verification line (bundle verification always happens;
    // gather info before calling into the library).
    let source_graph = allod_core::get_str(&bundle, "graph_id").unwrap_or("").to_string();
    let state_hash = bundle
        .get("checkpoint")
        .and_then(|cp| allod_core::get_str(cp, "state_hash"))
        .unwrap_or("")
        .to_string();
    let obj_count = bundle
        .get("objects")
        .and_then(Value::as_sequence)
        .map(|s| s.len())
        .unwrap_or(0);

    let Some(import_id) = import_id else {
        // Verify-only path: run all verification steps (signature, hashes,
        // Merkle proofs, audience) without creating any proposal.
        allod_graph::fed::verify_bundle(&graph, &bundle).map_err(|e| e.to_string())?;
        println!(
            "  ✓ bundle verified: {} objects prove membership in state {} of peer {}",
            obj_count,
            short(&state_hash),
            short(&source_graph)
        );
        return Ok(None);
    };

    // Import path.
    let admissions = allod_graph::fed::import(&graph, &bundle, by, import_id)
        .map_err(|e| e.to_string())?;

    println!(
        "  ✓ bundle verified: {} objects prove membership in state {} of peer {}",
        obj_count,
        short(&state_hash),
        short(&source_graph)
    );

    // Print the admission outcome (held as proposal or admitted directly).
    let mut held_hash: Option<String> = None;
    for admission in &admissions {
        crate::print_admission(admission);
        if let allod_graph::ops::Admission::Held { hash, .. } = admission {
            held_hash = Some(hash.clone());
        }
    }

    // Find the rev for the lineage line.
    let rev = bundle
        .get("objects")
        .and_then(Value::as_sequence)
        .and_then(|objs| {
            objs.iter().find_map(|obj| {
                let entry = obj.get("entry")?;
                (allod_core::get_str(entry, "id") == Some(import_id))
                    .then(|| allod_core::get_str(entry, "rev").unwrap_or("").to_string())
            })
        })
        .unwrap_or_default();

    println!(
        "      lineage: derived_from allod:{}/{}@{}",
        short(&source_graph),
        short(import_id),
        short(&rev)
    );

    Ok(held_hash)
}
