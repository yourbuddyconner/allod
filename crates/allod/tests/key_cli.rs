// crates/allod/tests/key_cli.rs
use std::process::Command;

fn repo_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn allod(args: &[&str], env_keys: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_allod"))
        .args(args)
        .env("ALLOD_KEYS_DIR", env_keys)
        .current_dir(repo_root())
        .output()
        .expect("run allod")
}

#[test]
fn init_migrate_where_roundtrip() {
    let root = std::env::temp_dir().join(format!("allod-keycli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let keys = root.join("keys");
    let g = root.join("g");
    let g_s = g.to_str().unwrap();

    // init writes .allod/.gitignore and creates the owner key at the XDG path.
    let out = allod(&["init", g_s, "--owner", "o"], &keys);
    assert!(out.status.success(), "init failed: {}", String::from_utf8_lossy(&out.stderr));
    let gi = std::fs::read_to_string(g.join(".allod/.gitignore")).unwrap();
    assert!(gi.contains("keys/"));
    assert!(!g.join(".allod/keys/o.yaml").exists(), "key must not be repo-local");

    // where reports the backend where the key was stored.
    // Key creation always defaults to the file backend (XDG path) on all platforms.
    // Resolution order differs: keychain is checked first on macOS, but creation targets file.
    let out = allod(&["key", "where", g_s, "--as", "o"], &keys);
    assert!(out.status.success(), "key where failed: {}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(text.contains("file:"), "default init must create at file backend, got: {text}");

    // Simulate a legacy repo-local key, then migrate it.
    let legacy_dir = g.join(".allod/keys");
    std::fs::create_dir_all(&legacy_dir).unwrap();
    // A fresh key under a different principal name so it doesn't collide:
    // generate via a second init in a scratch graph is overkill — write a
    // record through the library instead:
    // (integration tests may use the allod-core crate directly)
    let kp = allod_core::sign::Keypair::generate("legacy");
    std::fs::write(legacy_dir.join("legacy.yaml"),
        serde_yaml::to_string(&kp.to_yaml()).unwrap()).unwrap();
    let out = allod(&["key", "migrate", g_s, "--as", "legacy"], &keys);
    assert!(out.status.success(), "migrate failed: {}", String::from_utf8_lossy(&out.stderr));
    assert!(!legacy_dir.join("legacy.yaml").exists(), "legacy file must be moved");
    let out = allod(&["key", "where", g_s, "--as", "legacy"], &keys);
    assert!(out.status.success());

    // migrate again: nothing to do → error.
    let out = allod(&["key", "migrate", g_s, "--as", "legacy"], &keys);
    assert!(!out.status.success());

    let _ = std::fs::remove_dir_all(&root);
}
