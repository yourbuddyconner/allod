//! macOS Keychain key backend (§6.4.2).
//!
//! Keys are stored as generic passwords in the login keychain.
//! Service attribute: "allod" (production) or overridable for tests.
//! Account attribute: "<graph-dir-component>/<principal>" — the same
//! sanitization as the file backend so the two are interchangeable.
//!
//! Access control: the security-framework 3.x crate does not expose an API
//! to attach a SecAccessControl to a generic-password add (set_generic_password
//! takes only service, account, and password bytes).  The platform still gates
//! retrieval on keychain unlock; a signed binary with a keychain entitlement
//! provides the full biometry/passcode protection in production.  This deviation
//! from the ACL note in the spec brief is recorded in task-5-report.md.

use crate::keys::{graph_dir_component, KeyBackend, KeyHandle};
use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};
use zeroize::Zeroize;

// ─── KeychainBackend ──────────────────────────────────────────────────────────

/// A key backend backed by the macOS Keychain.
///
/// Stores keypair YAML as a generic password using service + account attributes.
/// The account is `"<graph-dir-component>/<principal>"`.
pub struct KeychainBackend {
    /// Keychain service attribute; `"allod"` in production, overridable for tests.
    pub service: String,
}

impl KeychainBackend {
    /// Construct with the production service name `"allod"`.
    pub fn new() -> Self {
        Self {
            service: "allod".into(),
        }
    }

    /// Account attribute for a graph-id + principal pair.
    fn account(graph_id: &str, principal: &str) -> String {
        format!("{}/{}", graph_dir_component(graph_id), principal)
    }

    /// Store a keypair's YAML record as a generic password.
    ///
    /// Errors if an item already exists for this account.
    pub fn store(
        &self,
        graph_id: &str,
        kp: &crate::sign::Keypair,
    ) -> Result<KeyHandle, String> {
        let account = Self::account(graph_id, &kp.name);

        // Reject if an item already exists — mirror the file backend's no-overwrite contract.
        if get_generic_password(&self.service, &account).is_ok() {
            return Err(format!(
                "key already exists in keychain (service={}, account={account})",
                self.service
            ));
        }

        let yaml_value = kp.to_yaml();
        let yaml_str = serde_yaml::to_string(&yaml_value)
            .map_err(|e| format!("cannot serialize keypair: {e}"))?;

        // Note: security_framework::passwords::set_generic_password does not
        // expose a SecAccessControl attachment API.  The keychain still gates
        // access on keychain unlock; production deployments should sign the
        // binary with the keychain entitlement for full system-level protection.
        set_generic_password(&self.service, &account, yaml_str.as_bytes())
            .map_err(|e| format!("keychain store failed: {e}"))?;

        Ok(KeyHandle::Keychain {
            account,
            name: kp.name.clone(),
        })
    }

    /// Delete the keychain item for `graph_id`/`principal`.
    ///
    /// Returns `Ok(())` even if the item did not exist (idempotent).
    pub fn delete(&self, graph_id: &str, principal: &str) -> Result<(), String> {
        let account = Self::account(graph_id, principal);
        match delete_generic_password(&self.service, &account) {
            Ok(()) => Ok(()),
            Err(e) => {
                // errSecItemNotFound = -25300: item was already gone, treat as success.
                if e.code() == -25300 {
                    Ok(())
                } else {
                    Err(format!("keychain delete failed: {e}"))
                }
            }
        }
    }

}

impl Default for KeychainBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyBackend for KeychainBackend {
    fn id(&self) -> &'static str {
        "keychain"
    }

    fn store_keypair(&self, graph_id: &str, kp: &crate::sign::Keypair) -> Result<(), String> {
        self.store(graph_id, kp).map(|_| ())
    }

    /// Find the key for `principal` in `graph_id` by probing the keychain.
    ///
    /// Returns `Err` (no prompt) if the item does not exist.
    fn resolve(&self, graph_id: &str, principal: &str) -> Result<KeyHandle, String> {
        let account = Self::account(graph_id, principal);
        get_generic_password(&self.service, &account)
            .map_err(|e| format!("key not found in keychain (service={}, account={account}): {e}", self.service))?;
        Ok(KeyHandle::Keychain {
            account,
            name: principal.to_string(),
        })
    }

    /// Sign `payload` using the key identified by `handle`.
    ///
    /// The retrieved YAML bytes are zeroized after use; the parsed Keypair is
    /// dropped at end of this call, keeping the secret lifetime to the minimum.
    fn sign(&self, handle: &KeyHandle, payload: &str) -> Result<String, String> {
        match handle {
            KeyHandle::Keychain { account, name: _ } => {
                // Derive graph_id+principal from the account string to load the right item.
                // The account format is "<graph-dir-component>/<principal>" and we have name.
                // We re-fetch by (service, account) directly.
                let bytes = get_generic_password(&self.service, account)
                    .map_err(|e| format!("key not found in keychain (service={}, account={account}): {e}", self.service))?;
                let mut bytes = bytes;
                let result = (|| {
                    let yaml_str = std::str::from_utf8(&bytes)
                        .map_err(|e| format!("keychain item is not UTF-8: {e}"))?;
                    let doc: serde_yaml::Value = serde_yaml::from_str(yaml_str)
                        .map_err(|e| format!("keychain item is not valid YAML: {e}"))?;
                    let kp = crate::sign::Keypair::from_yaml(&doc)?;
                    Ok(kp.sign(payload))
                })();
                bytes.zeroize();
                result
            }
            KeyHandle::File { .. } => {
                Err("keychain backend cannot use a file handle".to_string())
            }
        }
    }

    /// Return the hex-encoded public key for the key identified by `handle`.
    fn public(&self, handle: &KeyHandle) -> Result<String, String> {
        match handle {
            KeyHandle::Keychain { account, .. } => {
                let mut bytes = get_generic_password(&self.service, account)
                    .map_err(|e| format!("key not found in keychain (service={}, account={account}): {e}", self.service))?;
                let result = (|| {
                    let yaml_str = std::str::from_utf8(&bytes)
                        .map_err(|e| format!("keychain item is not UTF-8: {e}"))?;
                    let doc: serde_yaml::Value = serde_yaml::from_str(yaml_str)
                        .map_err(|e| format!("keychain item is not valid YAML: {e}"))?;
                    let kp = crate::sign::Keypair::from_yaml(&doc)?;
                    Ok(kp.public_hex())
                })();
                bytes.zeroize();
                result
            }
            KeyHandle::File { .. } => {
                Err("keychain backend cannot use a file handle".to_string())
            }
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::KeyBackend;

    /// Returns a test-scoped KeychainBackend only when ALLOD_KEYCHAIN_TESTS=1.
    /// Without the env var the tests self-skip (return None) so `cargo test --workspace`
    /// never touches the real keychain.
    fn test_service() -> Option<KeychainBackend> {
        if std::env::var("ALLOD_KEYCHAIN_TESTS").ok().as_deref() != Some("1") {
            return None;
        }
        Some(KeychainBackend {
            service: format!("allod-test-{}", std::process::id()),
        })
    }

    #[test]
    fn keychain_store_resolve_sign_roundtrip() {
        let Some(be) = test_service() else { return }; // skipped without opt-in
        let kp = crate::sign::Keypair::generate("kc");
        let public = kp.public_hex();
        let h = be.store("sha256:beef", &kp).unwrap();
        let sig = be.sign(&h, "sha256:11").unwrap();
        assert!(crate::sign::verify(&public, "sha256:11", &sig).is_ok());
        assert_eq!(be.public(&h).unwrap(), public);
        // Double-store errors; missing resolve errors.
        assert!(
            be.store("sha256:beef", &crate::sign::Keypair::generate("kc"))
                .is_err()
        );
        assert!(be.resolve("sha256:beef", "other").is_err());
        be.delete("sha256:beef", "kc").unwrap();
        assert!(be.resolve("sha256:beef", "kc").is_err());
    }

    #[test]
    fn keychain_backend_id() {
        // This test does NOT require the env var — it just checks the id string.
        let be = KeychainBackend::new();
        assert_eq!(be.id(), "keychain");
    }

    #[test]
    fn keychain_delete_absent_is_ok() {
        let Some(be) = test_service() else { return };
        // Deleting an item that was never created should succeed.
        assert!(be.delete("sha256:nonexistent", "nobody").is_ok());
    }
}
