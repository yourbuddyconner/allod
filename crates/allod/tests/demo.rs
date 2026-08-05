//! End-to-end test: the agent-memory demo runs, verification passes,
//! and a tampered log fails verification.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn run(keys_dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_allod"))
        .args(args)
        .env("ALLOD_KEYS_DIR", keys_dir)
        .current_dir(repo_root())
        .output()
        .expect("binary runs")
}

#[test]
fn demo_flow_verifies_and_tampering_fails() {
    let dir = std::env::temp_dir().join(format!("allod-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let keys_dir = std::env::temp_dir().join(format!("allod-test-keys-{}", std::process::id()));
    let dir_str = dir.to_string_lossy().to_string();

    let out = run(&keys_dir, &["demo", &dir_str, "--schema", "ontologies"]);
    assert!(
        out.status.success(),
        "demo failed:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("held as proposal"), "proposal must be held");
    assert!(stdout.contains("scratch-is-free"), "scratch rule must fire");
    assert!(stdout.contains("VERIFIED: 4 changesets"), "verify must pass");
    assert!(stdout.contains("degraded"), "evidence: none must be reported");

    let out = run(&keys_dir, &["verify", &dir_str]);
    assert!(out.status.success(), "standalone verify must pass");

    // Tamper with the admitted preference: change the statement text
    // in the stored changeset. Every verification level must notice.
    let changesets = dir.join(".allod/changesets");
    let mut tampered = false;
    for entry in fs::read_dir(&changesets).unwrap().flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        if !name.ends_with(".yaml") || name.ends_with(".evidence.yaml") {
            continue;
        }
        let text = fs::read_to_string(&path).unwrap();
        if text.contains("No meetings before 09:00") {
            fs::write(
                &path,
                text.replace("No meetings before 09:00", "All meetings before 09:00"),
            )
            .unwrap();
            tampered = true;
        }
    }
    assert!(tampered, "found the preference changeset to tamper with");
    let out = run(&keys_dir, &["verify", &dir_str]);
    assert!(
        !out.status.success(),
        "verify must fail after tampering:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&keys_dir);
}
