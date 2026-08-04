use allod_graph::md::{export, import};
use tempfile::TempDir;

mod common;

fn temp_dir() -> TempDir {
    tempfile::tempdir().expect("tempdir")
}

#[test]
fn export_creates_manifest_and_state_hash_matches() {
    let graph = common::init_memory_graph();
    // Add an agent and a note so there's at least one node.
    common::add_agent(&graph, "bot", "o");
    allod_graph::flows::note(&graph, "bot", "hello from test").expect("note");

    let td = temp_dir();
    let out = td.path().join("bundle");

    let report = export(&graph, &out).expect("export");

    // manifest.yaml must exist
    assert!(
        out.join(".allod/manifest.yaml").exists(),
        "manifest.yaml must be written"
    );
    assert!(report.files > 0, "at least one file exported");

    // state_hash in the report must equal the graph's own fold state hash
    let state_hash = graph.fold().expect("fold").state_hash().expect("state_hash");
    assert_eq!(
        report.state_hash, state_hash,
        "ExportReport.state_hash must match graph.fold().state_hash()"
    );
}

#[test]
fn unmodified_reimport_produces_zero_admissions() {
    let graph = common::init_memory_graph();
    common::add_agent(&graph, "bot2", "o");
    allod_graph::flows::note(&graph, "bot2", "round trip note").expect("note");

    let td = temp_dir();
    let out = td.path().join("bundle");

    export(&graph, &out).expect("export");

    // Re-import the unmodified bundle as the owner.
    let report = import(&graph, &out, "o").expect("import");

    assert!(
        report.admissions.is_empty(),
        "unmodified reimport must produce zero new admissions (got {:?})",
        report.admissions
    );
    assert!(
        report.skipped.is_empty(),
        "unmodified reimport must have no skipped files (got {:?})",
        report.skipped
    );
}
