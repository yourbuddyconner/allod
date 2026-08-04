//! CLI shim for federation (Part 9).
//!
//! Opens the graph from disk and delegates to `allod_graph::fed`; all
//! file I/O and stdout printing live here. The library (`allod_graph::fed`)
//! is pure: no filesystem access, no printing.

use allod_core::store::Graph;
use allod_graph::ops::{short, Admission};
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
        // Verify-only path: call import with a dummy id that won't match anything,
        // but we need the verification to run. Better: replicate the verification
        // path without the import step.
        // The library's import() always imports; for verify-only we rely on the
        // fact that the old CLI returned Ok(None) when no import_id was given.
        // We replicate that: verify by doing a no-op import (call with a sentinel
        // that will error at "import target not in bundle" — but we need the
        // verify steps to run first).
        // Simplest correct approach: call into the library with a known-absent id,
        // which will verify successfully then fail at "import target not in bundle".
        // Instead: re-implement the verify-only path using the same library internals.
        // For now, parse + verify by attempting to import a known-absent sentinel.
        // The library verifies ALL objects before looking for the import_id, so
        // a NotFound after successful verification is fine — but we'd surface an
        // error to the caller. We need to distinguish "bundle invalid" from
        // "target not found".
        //
        // Simplest: expose a `verify_bundle` function, OR handle it here by
        // catching the specific "import target not in bundle" error.
        //
        // To keep the library API minimal, we use the latter approach.
        let sentinel = "\x00not-a-real-id\x00";
        let result = allod_graph::fed::import(&graph, &bundle, by, sentinel)
            .map_err(|e| e.to_string());
        match result {
            Err(ref e) if e.contains("import target not in bundle") => {
                // Verification passed; only the import step was skipped.
                println!(
                    "  ✓ bundle verified: {} objects prove membership in state {} of peer {}",
                    obj_count,
                    short(&state_hash),
                    short(&source_graph)
                );
                return Ok(None);
            }
            Err(e) => return Err(e),
            Ok(_) => {
                // Sentinel matched something (shouldn't happen) — treat as verified.
                println!(
                    "  ✓ bundle verified: {} objects prove membership in state {} of peer {}",
                    obj_count,
                    short(&state_hash),
                    short(&source_graph)
                );
                return Ok(None);
            }
        }
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
        "      lineage: derived_from allod:{}/{import_id}@{}",
        short(&source_graph),
        short(&rev)
    );

    // Return the proposal hash if held, else None.
    match admissions.into_iter().next() {
        Some(Admission::Held { hash, .. }) => Ok(Some(hash)),
        _ => Ok(None),
    }
}
