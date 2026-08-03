//! Hashing (§1.7, §3.2.6): domain-separated SHA-256 over canonical
//! encodings, and the binary Merkle construction shared by the
//! operation tree and the state tree.
//!
//! Every hash carries the `sha256:` algorithm prefix. Domain
//! separation prepends `allod:<domain>:` to the payload, so a leaf
//! can never collide with an interior node or an object with a
//! changeset. The domains in use:
//!
//!   object      an object revision (§1.7), rev field omitted
//!   changeset   a changeset (§3.2.1), signature omitted, operations
//!               replaced by their Merkle root, intent by its hash
//!   intent      a changeset's intent text (§3.2.6)
//!   op-leaf     one operation's canonical encoding (§3.2.6)
//!   op-node     interior node of the operation tree
//!   state-leaf  one object's {kind, id, rev} entry (§1.7)
//!   state-node  interior node of the state tree
//!   package     an ontology package projection (§2.5). Until
//!               schema-as-object materialization ships, this content
//!               hash is what imports bind to
//!
//! Merkle construction (§1.7): binary over the leaves in order,
//! an odd node promoted unchanged, interior hash over the
//! concatenated raw 32-byte child digests.

use crate::canon::canonical_cbor;
use serde_yaml::Value;
use sha2::{Digest, Sha256};

/// Lowercase hex of arbitrary bytes.
pub fn hex_string(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Decode lowercase or uppercase hex.
pub fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    s.as_bytes()
        .chunks(2)
        .map(|chunk| {
            std::str::from_utf8(chunk)
                .ok()
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
        })
        .collect()
}

/// Plain SHA-256 of exact bytes, for content hashes (§1.5) and key
/// IDs (§6.2). No allod domain: these identities must match what
/// anyone computes over the raw bytes.
pub fn plain_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("sha256:{}", hex_string(&digest))
}

/// Domain-separated SHA-256, rendered as `sha256:<hex>`.
pub fn sha256_hex(domain: &str, payload: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"allod:");
    hasher.update(domain.as_bytes());
    hasher.update(b":");
    hasher.update(payload);
    let digest = hasher.finalize();
    format!("sha256:{}", hex_string(&digest))
}

/// Parse a `sha256:<hex>` string back to raw digest bytes.
pub fn digest_bytes(hash: &str) -> Option<[u8; 32]> {
    let hex = hash.strip_prefix("sha256:")?;
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        let s = std::str::from_utf8(chunk).ok()?;
        out[i] = u8::from_str_radix(s, 16).ok()?;
    }
    Some(out)
}

/// Root of a binary Merkle tree over leaf hashes, in order. An odd
/// node is promoted unchanged. Returns None for an empty tree.
pub fn merkle_root(leaves: &[String], node_domain: &str) -> Option<String> {
    if leaves.is_empty() {
        return None;
    }
    let mut level: Vec<String> = leaves.to_vec();
    while level.len() > 1 {
        let mut next = Vec::with_capacity(level.len().div_ceil(2));
        for pair in level.chunks(2) {
            if pair.len() == 2 {
                let mut payload = Vec::with_capacity(64);
                payload.extend_from_slice(&digest_bytes(&pair[0])?);
                payload.extend_from_slice(&digest_bytes(&pair[1])?);
                next.push(sha256_hex(node_domain, &payload));
            } else {
                next.push(pair[0].clone());
            }
        }
        level = next;
    }
    level.pop()
}

/// Hash a value's canonical encoding under a domain.
pub fn hash_value(domain: &str, value: &Value) -> Result<String, String> {
    Ok(sha256_hex(domain, &canonical_cbor(value)?))
}

/// The content hash an import binds to (§2.3): the canonical
/// encoding of the package's projection document, with its own
/// import hashes already resolved. Packages therefore hash in
/// dependency order.
pub fn package_hash(doc: &Value) -> Result<String, String> {
    hash_value("package", doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_carry_prefix_and_roundtrip() {
        let h = sha256_hex("object", b"payload");
        assert!(h.starts_with("sha256:"));
        assert_eq!(h.len(), 7 + 64);
        assert!(digest_bytes(&h).is_some());
    }

    #[test]
    fn domains_separate() {
        assert_ne!(sha256_hex("op-leaf", b"x"), sha256_hex("op-node", b"x"));
    }

    #[test]
    fn merkle_promotes_odd_nodes() {
        let a = sha256_hex("op-leaf", b"a");
        let b = sha256_hex("op-leaf", b"b");
        let c = sha256_hex("op-leaf", b"c");
        // A single leaf is its own root.
        assert_eq!(merkle_root(&[a.clone()], "op-node"), Some(a.clone()));
        // Three leaves: c is promoted beside hash(a, b).
        let ab = merkle_root(&[a.clone(), b.clone()], "op-node").unwrap();
        let abc = merkle_root(&[a, b, c.clone()], "op-node").unwrap();
        let expected = merkle_root(&[ab, c], "op-node").unwrap();
        assert_eq!(abc, expected);
        assert!(merkle_root(&[], "op-node").is_none());
    }
}
