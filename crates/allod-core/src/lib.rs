//! Core domain model for Allod tooling.
//!
//! This crate holds the parts of the specification that every tool
//! needs: the controlled vocabulary (Parts 1, 2, and 4), package and
//! taxonomy registries with reference resolution, the attribute type
//! grammar of §1.3, and a loader for projection-form YAML packages.
//! allod-lint builds on it, and reference implementation components
//! are expected to as well.

pub mod canon;
pub mod fold;
pub mod hash;
pub mod loader;
pub mod model;
pub mod policy;
pub mod registry;
pub mod sign;
pub mod store;
pub mod typeexpr;
pub mod vocab;

pub use canon::canonical_cbor;
pub use hash::{hash_value, merkle_root, package_hash, sha256_hex};
pub use loader::{load_dir, LoadIssue, Loaded};
pub use registry::{Package, Registry, Taxonomy};
pub use typeexpr::type_expr_errors;

use serde_yaml::Value;

/// Fetch a string field from a YAML mapping.
pub fn get_str<'a>(doc: &'a Value, key: &str) -> Option<&'a str> {
    doc.get(key).and_then(Value::as_str)
}

/// True when a hash value carries an algorithm prefix (§1.7).
pub fn has_algo_prefix(s: &str) -> bool {
    match s.find(':') {
        Some(i) if i > 0 => s[..i]
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
        _ => false,
    }
}

/// Strip a `@version` suffix from a reference.
pub fn bare(reference: &str) -> &str {
    reference.split('@').next().unwrap_or(reference)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn algo_prefixes() {
        assert!(has_algo_prefix("sha256:ab12"));
        assert!(has_algo_prefix("sha3-256:ab12"));
        assert!(!has_algo_prefix("notahash"));
        assert!(!has_algo_prefix(":dangling"));
        assert!(!has_algo_prefix("SHA256:upper"));
    }

    #[test]
    fn bare_strips_versions() {
        assert_eq!(bare("core/Person@2"), "core/Person");
        assert_eq!(bare("core/Person"), "core/Person");
    }
}
