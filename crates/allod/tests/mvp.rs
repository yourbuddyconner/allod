//! The Appendix A acceptance test, executed end to end through the
//! CLI. Pass means the MVP claim is a passing test, not an assertion.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn run(args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_allod"))
        .args(args)
        .current_dir(repo_root())
        .output()
        .expect("binary runs");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

fn ok(args: &[&str], must_contain: &[&str]) -> String {
    let (success, text) = run(args);
    assert!(success, "command {args:?} failed:\n{text}");
    for needle in must_contain {
        assert!(text.contains(needle), "missing {needle:?} in:\n{text}");
    }
    text
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
fn appendix_a_acceptance() {
    let base = std::env::temp_dir().join(format!("allod-mvp-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    let p = |name: &str| base.join(name).to_string_lossy().to_string();
    let mem = p("memory");
    let sch = "ontologies";

    // Steps 1, 4 (memory form), and the accept path: the jarvis flow.
    ok(
        &["demo", &mem, "--schema", sch],
        &["scratch-is-free", "held as proposal", "VERIFIED: 4 changesets", "degraded"],
    );

    // Step 6 setup + the note id, via the markdown bundle (§7.2).
    let bundle = p("bundle");
    ok(&["export-md", &mem, &bundle, "--schema", sch], &["exported 4 nodes"]);
    let scratch = base.join("bundle/workspace/scratch");
    let note_id = fs::read_dir(&scratch)
        .unwrap()
        .flatten()
        .find_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.strip_suffix(".md").map(String::from)
        })
        .expect("scratch note exported");

    // Step 5, the reject path: a model-assisted classification into a
    // sensitivity/private region, without an attestation envelope.
    ok(
        &[
            "classify", &mem, &note_id, "life/health@1", "--as", "jarvis",
            "--basis", "model-assisted", "--schema", sch,
        ],
        &["held as proposal", "attestation from class indexer"],
    );
    let proposals_dir = base.join("memory/.allod/proposals");
    let proposal = fs::read_dir(&proposals_dir)
        .unwrap()
        .flatten()
        .find_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            (!name.ends_with(".evidence.yaml"))
                .then(|| format!("sha256:{}", name.strip_suffix(".yaml").unwrap()))
        })
        .expect("held proposal on disk");
    ok(
        &["reject", &mem, &proposal, "--as", "conner", "--schema", sch],
        &["rejected", "stay auditable"],
    );
    let evidence = fs::read_to_string(
        proposals_dir.join(format!(
            "{}.evidence.yaml",
            proposal.strip_prefix("sha256:").unwrap()
        )),
    )
    .unwrap();
    assert!(evidence.contains("reject"), "rejection is recorded");

    // Step 6: unmodified round trip, then a hand edit as a proposal.
    ok(
        &["import-md", &mem, &bundle, "--as", "jarvis", "--schema", sch],
        &["round trip verified"],
    );
    let pref = base.join("bundle/work");
    let pref_file = fs::read_dir(&pref)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().and_then(|e| e.to_str()) == Some("md"))
        .expect("preference exported");
    let text = fs::read_to_string(&pref_file).unwrap();
    fs::write(&pref_file, text.replace("strength: soft", "strength: hard")).unwrap();
    ok(
        &["import-md", &mem, &bundle, "--as", "jarvis", "--schema", sch],
        &["1 edited", "held as proposal"],
    );

    // Step 7: replay on a "second machine" (a copy), plus a
    // checkpoint that replay verifies (§3.2.5).
    let replay = base.join("memory-replayed");
    copy_dir(&base.join("memory"), &replay);
    ok(
        &["verify", &replay.to_string_lossy(), "--schema", sch],
        &["VERIFIED"],
    );
    ok(&["checkpoint", &mem, "--as", "conner", "--schema", sch], &["checkpoint"]);
    ok(
        &["verify", &mem, "--schema", sch],
        &["verified against replay", "VERIFIED"],
    );

    // Steps 2, 3, 4, 8: derivation, semantic diff, governed
    // classification, and the simulated-evidence envelope.
    ok(
        &["demo-code", &p("code"), "--schema", sch],
        &[
            "deterministic-indexer-writes",
            "security-region-owner-signoff",
            "semantic diff",
            "inbound caller(s): refund",
            "simulated measurement",
            "VERIFIED: 6 changesets",
        ],
    );

    // Step 9: federation — grant, proof-carrying bundle, governed
    // import with lineage, revocation refused.
    ok(
        &["demo-federation", &p("fed-a"), &p("fed-b"), "--schema", sch],
        &[
            "bundle verified",
            "held as proposal",
            "lineage: derived_from allod:",
            "VERIFIED: 3 changesets",
            "refused",
        ],
    );

    let _ = fs::remove_dir_all(&base);
}
