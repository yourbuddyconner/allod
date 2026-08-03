//! Signatures (Part 6): Ed25519 keypairs, signing, and verification.
//!
//! A signature covers the ASCII bytes of the hash string it signs
//! (`sha256:<hex>`), rendered `sig:ed25519:<hex>`. The key ID (§6.2)
//! is the plain SHA-256 of the raw 32-byte public key. Keys serialize
//! to a small YAML record; the reference plain-keypair profile
//! (§6.4.1) is a file on disk, and the demo stores secrets in the
//! clear — a real deployment does not.

use crate::hash::{hex_decode, hex_string, plain_sha256};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde_yaml::{Mapping, Value};

pub struct Keypair {
    pub name: String,
    signing: SigningKey,
}

impl Keypair {
    pub fn generate(name: &str) -> Keypair {
        Keypair {
            name: name.to_string(),
            signing: SigningKey::generate(&mut rand::rngs::OsRng),
        }
    }

    /// A keypair from a fixed secret, for deterministic test vectors
    /// (Appendix H). Never for real graphs.
    pub fn from_secret_hex(name: &str, secret_hex: &str) -> Result<Keypair, String> {
        let bytes = hex_decode(secret_hex).ok_or("secret is not hex")?;
        let bytes: [u8; 32] = bytes.try_into().map_err(|_| "secret must be 32 bytes")?;
        Ok(Keypair {
            name: name.to_string(),
            signing: SigningKey::from_bytes(&bytes),
        })
    }

    /// Key ID (§6.2): plain SHA-256 of the raw public key bytes.
    pub fn key_id(&self) -> String {
        plain_sha256(self.signing.verifying_key().as_bytes())
    }

    pub fn public_hex(&self) -> String {
        hex_string(self.signing.verifying_key().as_bytes())
    }

    /// Sign a hash string, e.g. a changeset hash (§3.2.1).
    pub fn sign(&self, message: &str) -> String {
        let sig = self.signing.sign(message.as_bytes());
        format!("sig:ed25519:{}", hex_string(&sig.to_bytes()))
    }

    pub fn to_yaml(&self) -> Value {
        let mut map = Mapping::new();
        map.insert(Value::String("name".into()), Value::String(self.name.clone()));
        map.insert(Value::String("key_id".into()), Value::String(self.key_id()));
        map.insert(
            Value::String("algorithm".into()),
            Value::String("ed25519".into()),
        );
        map.insert(
            Value::String("public".into()),
            Value::String(self.public_hex()),
        );
        map.insert(
            Value::String("secret".into()),
            Value::String(hex_string(&self.signing.to_bytes())),
        );
        Value::Mapping(map)
    }

    pub fn from_yaml(doc: &Value) -> Result<Keypair, String> {
        let name = crate::get_str(doc, "name").ok_or("key record needs a name")?;
        let secret = crate::get_str(doc, "secret").ok_or("key record needs a secret")?;
        let bytes = hex_decode(secret).ok_or("secret is not hex")?;
        let bytes: [u8; 32] = bytes.try_into().map_err(|_| "secret must be 32 bytes")?;
        Ok(Keypair {
            name: name.to_string(),
            signing: SigningKey::from_bytes(&bytes),
        })
    }
}

/// Verify a `sig:ed25519:<hex>` signature over a hash string with a
/// hex-encoded public key.
pub fn verify(public_hex: &str, message: &str, signature: &str) -> Result<(), String> {
    let sig_hex = signature
        .strip_prefix("sig:ed25519:")
        .ok_or("unsupported signature form")?;
    let sig_bytes = hex_decode(sig_hex).ok_or("signature is not hex")?;
    let sig_bytes: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| "signature must be 64 bytes")?;
    let key_bytes = hex_decode(public_hex).ok_or("public key is not hex")?;
    let key_bytes: [u8; 32] = key_bytes
        .try_into()
        .map_err(|_| "public key must be 32 bytes")?;
    let key = VerifyingKey::from_bytes(&key_bytes).map_err(|e| e.to_string())?;
    key.verify(message.as_bytes(), &Signature::from_bytes(&sig_bytes))
        .map_err(|_| "signature does not verify".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_verify_roundtrip() {
        let kp = Keypair::generate("test");
        let msg = "sha256:00ff";
        let sig = kp.sign(msg);
        assert!(verify(&kp.public_hex(), msg, &sig).is_ok());
        assert!(verify(&kp.public_hex(), "sha256:other", &sig).is_err());
    }

    #[test]
    fn yaml_roundtrip_preserves_identity() {
        let kp = Keypair::generate("test");
        let restored = Keypair::from_yaml(&kp.to_yaml()).unwrap();
        assert_eq!(kp.key_id(), restored.key_id());
        let sig = restored.sign("sha256:aa");
        assert!(verify(&kp.public_hex(), "sha256:aa", &sig).is_ok());
    }
}
