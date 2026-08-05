//! Key backends (§6.4): KeyHandle, KeyBackend trait, FileBackend, and Signer.
//!
//! Keys are stored as YAML records (plain-keypair profile §6.4.1).
//! The file backend reads from a graph-id-keyed XDG directory and
//! optionally from legacy in-repo `.allod/keys/` fallback directories.
//! Windows is out of scope; `platform_default` relies on `$HOME`.

use crate::hash::{hex_decode, plain_sha256};

// ─── KeyHandle ────────────────────────────────────────────────────────────────

/// A located key that a [`KeyBackend`] can operate on.
pub enum KeyHandle {
    File {
        path: std::path::PathBuf,
        name: String,
    },
    #[cfg(target_os = "macos")]
    Keychain { account: String, name: String },
}

impl KeyHandle {
    /// The principal name stored in this key record.
    pub fn name(&self) -> &str {
        match self {
            KeyHandle::File { name, .. } => name,
            #[cfg(target_os = "macos")]
            KeyHandle::Keychain { name, .. } => name,
        }
    }

    /// Human-readable location for `allod key where` output.
    pub fn describe(&self) -> String {
        match self {
            KeyHandle::File { path, .. } => format!("file: {}", path.display()),
            #[cfg(target_os = "macos")]
            KeyHandle::Keychain { account, name } => {
                format!("keychain: account={account} name={name}")
            }
        }
    }
}

// ─── KeyBackend ───────────────────────────────────────────────────────────────

/// A storage backend that can locate and use signing keys.
pub trait KeyBackend {
    /// Short identifier for this backend, e.g. `"file"` or `"keychain"`.
    fn id(&self) -> &'static str;
    /// Find the key for `principal` within `graph_id`, returning a handle.
    fn resolve(&self, graph_id: &str, principal: &str) -> Result<KeyHandle, String>;
    /// Sign `payload` using the key identified by `handle`.
    fn sign(&self, handle: &KeyHandle, payload: &str) -> Result<String, String>;
    /// Return the hex-encoded public key for the key identified by `handle`.
    fn public(&self, handle: &KeyHandle) -> Result<String, String>;
}

// ─── graph_dir_component ──────────────────────────────────────────────────────

/// Filesystem-safe directory component from a graph id.
///
/// Strips the `sha256:` prefix if present, then maps any character
/// outside `[A-Za-z0-9._-]` to `'-'`.
pub fn graph_dir_component(graph_id: &str) -> String {
    let stripped = graph_id.strip_prefix("sha256:").unwrap_or(graph_id);
    stripped
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

// ─── FileBackend ──────────────────────────────────────────────────────────────

/// A key backend that stores keys as YAML files on disk.
///
/// New keys are written to `<create_dir>/<graph-dir-component>/<principal>.yaml`.
/// Reads also check each directory in `fallbacks` at `<fallback>/<principal>.yaml`
/// (the legacy in-repo `.allod/keys/` layout — not graph-id-keyed).
pub struct FileBackend {
    /// Where new keys are created: `<create_dir>/<graph-id-component>/<principal>.yaml`
    pub create_dir: std::path::PathBuf,
    /// Read-only fallback dirs, tried in order, layout `<dir>/<principal>.yaml`
    /// (the legacy in-repo `.allod/keys/` layout — NOT graph-id-keyed).
    pub fallbacks: Vec<std::path::PathBuf>,
}

impl FileBackend {
    /// Resolve the platform default create dir:
    /// `ALLOD_KEYS_DIR` > `$XDG_DATA_HOME/allod/keys` > `~/.local/share/allod/keys`.
    ///
    /// Windows is out of scope; home directory is read from `$HOME`.
    pub fn platform_default(fallbacks: Vec<std::path::PathBuf>) -> FileBackend {
        let create_dir = if let Ok(d) = std::env::var("ALLOD_KEYS_DIR") {
            std::path::PathBuf::from(d)
        } else if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
            std::path::PathBuf::from(xdg).join("allod/keys")
        } else {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            std::path::PathBuf::from(home).join(".local/share/allod/keys")
        };
        FileBackend {
            create_dir,
            fallbacks,
        }
    }

    /// Persist an in-memory keypair at the create path.
    ///
    /// Creates parent directories as needed. Errors if the target file
    /// already exists — keys are never silently overwritten.
    pub fn store(
        &self,
        graph_id: &str,
        kp: &crate::sign::Keypair,
    ) -> Result<KeyHandle, String> {
        let dir = self.create_dir.join(graph_dir_component(graph_id));
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("cannot create key dir {}: {e}", dir.display()))?;
        let path = dir.join(format!("{}.yaml", kp.name));
        let yaml_value = kp.to_yaml();
        let contents = serde_yaml::to_string(&yaml_value)
            .map_err(|e| format!("cannot serialize key: {e}"))?;
        // Use create_new so the no-overwrite check is atomic (no TOCTOU).
        use std::io::Write as _;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    format!("key already exists at {}", path.display())
                } else {
                    format!("cannot create key file {}: {e}", path.display())
                }
            })?;
        file.write_all(contents.as_bytes())
            .map_err(|e| format!("cannot write key file {}: {e}", path.display()))?;
        Ok(KeyHandle::File {
            path,
            name: kp.name.clone(),
        })
    }

    /// Load a `Keypair` from the YAML file pointed to by a `File` handle.
    fn load_keypair(path: &std::path::Path) -> Result<crate::sign::Keypair, String> {
        let contents = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read key file {}: {e}", path.display()))?;
        let doc: serde_yaml::Value = serde_yaml::from_str(&contents)
            .map_err(|e| format!("cannot parse key YAML {}: {e}", path.display()))?;
        crate::sign::Keypair::from_yaml(&doc)
    }
}

impl KeyBackend for FileBackend {
    fn id(&self) -> &'static str {
        "file"
    }

    /// Search `<create_dir>/<graph_dir_component>/<principal>.yaml`, then
    /// each `<fallback>/<principal>.yaml`. First existing file wins.
    fn resolve(&self, graph_id: &str, principal: &str) -> Result<KeyHandle, String> {
        let filename = format!("{principal}.yaml");
        let primary = self
            .create_dir
            .join(graph_dir_component(graph_id))
            .join(&filename);

        let mut candidates = vec![primary];
        for fb in &self.fallbacks {
            candidates.push(fb.join(&filename));
        }

        for path in &candidates {
            if path.is_file() {
                return Ok(KeyHandle::File {
                    path: path.clone(),
                    name: principal.to_string(),
                });
            }
        }

        Err(format!(
            "no key for {principal} (searched {} locations)",
            candidates.len()
        ))
    }

    fn sign(&self, handle: &KeyHandle, payload: &str) -> Result<String, String> {
        match handle {
            KeyHandle::File { path, .. } => {
                let kp = Self::load_keypair(path)?;
                Ok(kp.sign(payload))
            }
            #[cfg(target_os = "macos")]
            KeyHandle::Keychain { .. } => {
                Err("file backend cannot use a keychain handle".to_string())
            }
        }
    }

    fn public(&self, handle: &KeyHandle) -> Result<String, String> {
        match handle {
            KeyHandle::File { path, .. } => {
                let kp = Self::load_keypair(path)?;
                Ok(kp.public_hex())
            }
            #[cfg(target_os = "macos")]
            KeyHandle::Keychain { .. } => {
                Err("file backend cannot use a keychain handle".to_string())
            }
        }
    }
}

// ─── Signer ───────────────────────────────────────────────────────────────────

enum SignerInner<'a> {
    Local(crate::sign::Keypair),
    Backend {
        backend: &'a dyn KeyBackend,
        handle: KeyHandle,
    },
}

/// A signing principal backed by either an in-memory keypair or a key backend.
pub struct Signer<'a> {
    inner: SignerInner<'a>,
}

impl<'a> Signer<'a> {
    /// Wrap an in-memory keypair; the returned `Signer` has `'static` lifetime.
    pub fn local(kp: crate::sign::Keypair) -> Signer<'static> {
        Signer {
            inner: SignerInner::Local(kp),
        }
    }

    /// Wrap a backend + handle pair.
    pub fn from_backend(backend: &'a dyn KeyBackend, handle: KeyHandle) -> Signer<'a> {
        Signer {
            inner: SignerInner::Backend { backend, handle },
        }
    }

    /// The principal name associated with this signer.
    pub fn name(&self) -> &str {
        match &self.inner {
            SignerInner::Local(kp) => &kp.name,
            SignerInner::Backend { handle, .. } => handle.name(),
        }
    }

    /// Sign `message`, returning a `sig:ed25519:<hex>` string.
    pub fn sign(&self, message: &str) -> Result<String, String> {
        match &self.inner {
            SignerInner::Local(kp) => Ok(kp.sign(message)),
            SignerInner::Backend { backend, handle } => backend.sign(handle, message),
        }
    }

    /// Hex-encoded public key for this signer.
    pub fn public_hex(&self) -> Result<String, String> {
        match &self.inner {
            SignerInner::Local(kp) => Ok(kp.public_hex()),
            SignerInner::Backend { backend, handle } => backend.public(handle),
        }
    }

    /// Plain SHA-256 of the raw public key bytes (§6.2) — matches `Keypair::key_id`.
    pub fn key_id(&self) -> Result<String, String> {
        match &self.inner {
            SignerInner::Local(kp) => Ok(kp.key_id()),
            SignerInner::Backend { .. } => {
                let public = self.public_hex()?;
                let bytes = hex_decode(&public).ok_or("public key is not hex")?;
                Ok(plain_sha256(&bytes))
            }
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_dir_component_sanitizes() {
        assert_eq!(graph_dir_component("sha256:ab/cd:ef"), "ab-cd-ef");
        assert_eq!(graph_dir_component("plain-id_1.2"), "plain-id_1.2");
    }

    #[test]
    fn file_backend_creates_resolves_signs() {
        let tmp = std::env::temp_dir().join(format!("allod-keys-t1-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let be = FileBackend { create_dir: tmp.clone(), fallbacks: vec![] };
        let kp = crate::sign::Keypair::generate("alice");
        let expected_public = kp.public_hex();
        let _handle = be.store("sha256:feed", &kp).unwrap();
        // Path layout: <create_dir>/feed/alice.yaml
        assert!(tmp.join("feed").join("alice.yaml").is_file());
        let resolved = be.resolve("sha256:feed", "alice").unwrap();
        assert_eq!(resolved.name(), "alice");
        assert_eq!(be.public(&resolved).unwrap(), expected_public);
        let sig = be.sign(&resolved, "sha256:00ff").unwrap();
        assert!(crate::sign::verify(&expected_public, "sha256:00ff", &sig).is_ok());
        // Missing principal errors.
        assert!(be.resolve("sha256:feed", "bob").is_err());
        // Never overwrite.
        assert!(be.store("sha256:feed", &crate::sign::Keypair::generate("alice")).is_err());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn file_backend_reads_legacy_fallback() {
        let tmp = std::env::temp_dir().join(format!("allod-keys-t2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let legacy = tmp.join("repo/.allod/keys");
        std::fs::create_dir_all(&legacy).unwrap();
        let kp = crate::sign::Keypair::generate("carol");
        std::fs::write(
            legacy.join("carol.yaml"),
            serde_yaml::to_string(&kp.to_yaml()).unwrap(),
        ).unwrap();
        let be = FileBackend { create_dir: tmp.join("xdg"), fallbacks: vec![legacy.clone()] };
        let h = be.resolve("sha256:anything", "carol").unwrap();
        assert_eq!(be.public(&h).unwrap(), kp.public_hex());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn signer_local_and_backend_parity() {
        let tmp = std::env::temp_dir().join(format!("allod-keys-t3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let be = FileBackend { create_dir: tmp.clone(), fallbacks: vec![] };
        let kp = crate::sign::Keypair::from_secret_hex(
            "dana",
            "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
        ).unwrap();
        let expected_sig = kp.sign("sha256:aa");
        let expected_kid = kp.key_id();
        let handle = be.store("gid", &kp).unwrap();
        let s_local = Signer::local(kp);
        let s_backend = Signer::from_backend(&be, handle);
        assert_eq!(s_local.sign("sha256:aa").unwrap(), expected_sig);
        assert_eq!(s_backend.sign("sha256:aa").unwrap(), expected_sig);
        assert_eq!(s_local.key_id().unwrap(), expected_kid);
        assert_eq!(s_backend.key_id().unwrap(), expected_kid);
        assert_eq!(s_local.name(), "dana");
        assert_eq!(s_backend.name(), "dana");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
