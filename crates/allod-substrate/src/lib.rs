//! The abstract changeset substrate (§3.1): the four properties every
//! substrate provides — content-addressed revisions, a parent-pointer
//! DAG, signable authorship, and deterministic state. Parts 4 to 6 of
//! the spec are written against this interface; `NativeSubstrate`
//! adapts the native log (§3.2) and a git binding (§3.3) follows in
//! its own crate.

pub mod conformance;
pub mod native;

/// A revision hash, algorithm-prefixed (§1.7), e.g. `sha256:…`.
pub type RevHash = String;
/// A branch-head name, e.g. `HEAD` or `refs/heads/main`.
pub type RefName = String;

/// One revision's envelope, substrate-neutral (§3.1).
pub struct Revision {
    pub hash: RevHash,
    /// At least one entry except genesis; more than one is a merge.
    pub parents: Vec<RevHash>,
    /// Substrate-specific author record, preserved verbatim
    /// (native: principal-ref + key id; git: committer + key).
    pub author: serde_yaml::Value,
    pub timestamp: Option<String>,
    pub signed: bool,
}

/// Outcome of checking a revision's authorship signature (§3.1 property 3).
pub enum AuthorVerdict {
    Verified { principal: String, key_id: String },
    Unsigned,
    Failed(String),
}

/// The §3.1 interface. Implementations MUST enforce content
/// addressing on read: `revision(h)` fails when the stored content
/// does not recompute to `h`.
pub trait Substrate {
    fn revision(&self, hash: &str) -> Result<Revision, String>;
    /// The revision's operation set, deterministic (§3.2.2 native,
    /// §3.3 git tree diff).
    fn operation_set(&self, hash: &str) -> Result<Vec<serde_yaml::Value>, String>;
    /// State hash at this revision (§3.1 property 4).
    fn state_hash(&self, hash: &str) -> Result<String, String>;
    fn heads(&self) -> Result<Vec<(RefName, RevHash)>, String>;
    fn verify_authorship(&self, hash: &str) -> Result<AuthorVerdict, String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    // The trait must be object-safe: consumers hold `&dyn Substrate`.
    fn takes_dyn(_s: &dyn Substrate) {}

    struct Empty;
    impl Substrate for Empty {
        fn revision(&self, _hash: &str) -> Result<Revision, String> {
            Err("empty".into())
        }
        fn operation_set(&self, _hash: &str) -> Result<Vec<serde_yaml::Value>, String> {
            Err("empty".into())
        }
        fn state_hash(&self, _hash: &str) -> Result<String, String> {
            Err("empty".into())
        }
        fn heads(&self) -> Result<Vec<(RefName, RevHash)>, String> {
            Ok(vec![])
        }
        fn verify_authorship(&self, _hash: &str) -> Result<AuthorVerdict, String> {
            Ok(AuthorVerdict::Unsigned)
        }
    }

    #[test]
    fn trait_is_object_safe_and_types_construct() {
        takes_dyn(&Empty);
        let rev = Revision {
            hash: "sha256:aa".into(),
            parents: vec![],
            author: serde_yaml::Value::Null,
            timestamp: None,
            signed: false,
        };
        assert_eq!(rev.parents.len(), 0);
        assert!(matches!(AuthorVerdict::Unsigned, AuthorVerdict::Unsigned));
    }
}
