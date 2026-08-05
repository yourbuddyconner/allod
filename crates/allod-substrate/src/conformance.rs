//! Substrate conformance (§3.1): one suite, run against every
//! implementation. Native runs it in this workspace; the git binding
//! (milestone 2) runs the same checks.

use crate::{AuthorVerdict, Substrate};
use std::collections::BTreeSet;

/// Walk limit: a conformance fixture is small; hitting this means a
/// broken parent DAG, not a big history.
const MAX_WALK: usize = 10_000;

/// Check the four §3.1 properties against a live fixture.
///
/// Preconditions on the fixture: at least one head; the head
/// revision's operation set is non-empty and creates at least one
/// object (so head state differs from parent state); `signed_rev`
/// names a revision whose authorship must verify.
pub fn check_conformance(sub: &dyn Substrate, signed_rev: &str) -> Result<(), String> {
    let heads = sub.heads()?;
    if heads.is_empty() {
        return Err("conformance: substrate reports no heads".into());
    }

    for (name, head) in &heads {
        // Property 1: content-addressed revisions.
        let rev = sub.revision(head)?;
        if rev.hash != *head {
            return Err(format!(
                "conformance: revision identity mismatch at head {name}: asked {head}, got {}",
                rev.hash
            ));
        }
        if !allod_core::has_algo_prefix(&rev.hash) {
            return Err(format!(
                "conformance: revision hash {} lacks an algorithm prefix (§1.7)",
                rev.hash
            ));
        }

        // Property 2: parent-pointer DAG — walk to genesis, no cycles.
        let mut frontier = vec![head.clone()];
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut steps = 0usize;
        while let Some(h) = frontier.pop() {
            if !seen.insert(h.clone()) {
                return Err(format!("conformance: parent DAG cycle at {h}"));
            }
            steps += 1;
            if steps > MAX_WALK {
                return Err("conformance: parent walk exceeded limit (broken DAG?)".into());
            }
            let r = sub.revision(&h)?;
            for p in r.parents {
                if seen.contains(&p) {
                    return Err(format!("conformance: parent DAG cycle at {p}"));
                }
                frontier.push(p);
            }
        }

        // Property 4: deterministic state.
        let s1 = sub.state_hash(head)?;
        let s2 = sub.state_hash(head)?;
        if s1 != s2 {
            return Err(format!(
                "conformance: state hash at {head} is not deterministic ({s1} vs {s2})"
            ));
        }
        let head_rev = sub.revision(head)?;
        if let Some(parent) = head_rev.parents.first() {
            if !sub.operation_set(head)?.is_empty() {
                let sp = sub.state_hash(parent)?;
                if sp == s1 {
                    return Err(format!(
                        "conformance: head {head} has operations but the same state as its parent"
                    ));
                }
            }
        }
    }

    // Property 3: signable authorship.
    match sub.verify_authorship(signed_rev)? {
        AuthorVerdict::Verified { .. } => Ok(()),
        AuthorVerdict::Unsigned => Err(format!(
            "conformance: authorship of {signed_rev} is unsigned; fixture promised a signed revision"
        )),
        AuthorVerdict::Failed(reason) => Err(format!(
            "conformance: authorship of {signed_rev} failed: {reason}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AuthorVerdict, Revision, Substrate};
    use std::collections::BTreeMap;

    /// Minimal well-behaved substrate: a two-revision chain.
    struct Fake {
        revs: BTreeMap<String, (Vec<String>, String)>, // hash -> (parents, state_hash)
        head: String,
    }

    impl Fake {
        fn good() -> Fake {
            let mut revs = BTreeMap::new();
            revs.insert("sha256:genesis".to_string(), (vec![], "sha256:s0".to_string()));
            revs.insert(
                "sha256:tip".to_string(),
                (vec!["sha256:genesis".to_string()], "sha256:s1".to_string()),
            );
            Fake { revs, head: "sha256:tip".to_string() }
        }
    }

    impl Substrate for Fake {
        fn revision(&self, hash: &str) -> Result<Revision, String> {
            let (parents, _) = self.revs.get(hash).ok_or("unknown revision")?;
            Ok(Revision {
                hash: hash.to_string(),
                parents: parents.clone(),
                author: serde_yaml::Value::Null,
                timestamp: None,
                signed: true,
            })
        }
        fn operation_set(&self, _hash: &str) -> Result<Vec<serde_yaml::Value>, String> {
            Ok(vec![serde_yaml::from_str("{ create: { kind: node, id: a } }").unwrap()])
        }
        fn state_hash(&self, hash: &str) -> Result<String, String> {
            Ok(self.revs.get(hash).ok_or("unknown revision")?.1.clone())
        }
        fn heads(&self) -> Result<Vec<(String, String)>, String> {
            Ok(vec![("HEAD".to_string(), self.head.clone())])
        }
        fn verify_authorship(&self, _hash: &str) -> Result<AuthorVerdict, String> {
            Ok(AuthorVerdict::Verified { principal: "principal:o".into(), key_id: "k".into() })
        }
    }

    #[test]
    fn good_substrate_conforms() {
        let fake = Fake::good();
        check_conformance(&fake, "sha256:tip").unwrap();
    }

    #[test]
    fn cyclic_parent_dag_fails() {
        let mut fake = Fake::good();
        // tip's parent points back at tip: a cycle.
        fake.revs.insert(
            "sha256:genesis".to_string(),
            (vec!["sha256:tip".to_string()], "sha256:s0".to_string()),
        );
        let err = check_conformance(&fake, "sha256:tip").unwrap_err();
        assert!(err.contains("cycle"), "got: {err}");
    }

    #[test]
    fn wrong_hash_identity_fails() {
        struct Lying(Fake);
        impl Substrate for Lying {
            fn revision(&self, hash: &str) -> Result<Revision, String> {
                let mut r = self.0.revision(hash)?;
                r.hash = "sha256:other".to_string(); // content-address violation
                Ok(r)
            }
            fn operation_set(&self, h: &str) -> Result<Vec<serde_yaml::Value>, String> {
                self.0.operation_set(h)
            }
            fn state_hash(&self, h: &str) -> Result<String, String> { self.0.state_hash(h) }
            fn heads(&self) -> Result<Vec<(String, String)>, String> { self.0.heads() }
            fn verify_authorship(&self, h: &str) -> Result<AuthorVerdict, String> {
                self.0.verify_authorship(h)
            }
        }
        let err = check_conformance(&Lying(Fake::good()), "sha256:tip").unwrap_err();
        assert!(err.contains("identity"), "got: {err}");
    }

    #[test]
    fn nondeterministic_state_fails() {
        struct Flaky { inner: Fake, calls: std::cell::Cell<u32> }
        impl Substrate for Flaky {
            fn revision(&self, h: &str) -> Result<Revision, String> { self.inner.revision(h) }
            fn operation_set(&self, h: &str) -> Result<Vec<serde_yaml::Value>, String> {
                self.inner.operation_set(h)
            }
            fn state_hash(&self, h: &str) -> Result<String, String> {
                let n = self.calls.get();
                self.calls.set(n + 1);
                Ok(format!("sha256:varies-{n}-{h}"))
            }
            fn heads(&self) -> Result<Vec<(String, String)>, String> { self.inner.heads() }
            fn verify_authorship(&self, h: &str) -> Result<AuthorVerdict, String> {
                self.inner.verify_authorship(h)
            }
        }
        let flaky = Flaky { inner: Fake::good(), calls: std::cell::Cell::new(0) };
        let err = check_conformance(&flaky, "sha256:tip").unwrap_err();
        assert!(err.contains("deterministic"), "got: {err}");
    }

    #[test]
    fn unverified_authorship_fails() {
        struct NoSig(Fake);
        impl Substrate for NoSig {
            fn revision(&self, h: &str) -> Result<Revision, String> { self.0.revision(h) }
            fn operation_set(&self, h: &str) -> Result<Vec<serde_yaml::Value>, String> {
                self.0.operation_set(h)
            }
            fn state_hash(&self, h: &str) -> Result<String, String> { self.0.state_hash(h) }
            fn heads(&self) -> Result<Vec<(String, String)>, String> { self.0.heads() }
            fn verify_authorship(&self, _h: &str) -> Result<AuthorVerdict, String> {
                Ok(AuthorVerdict::Failed("bad signature".into()))
            }
        }
        let err = check_conformance(&NoSig(Fake::good()), "sha256:tip").unwrap_err();
        assert!(err.contains("authorship"), "got: {err}");
    }
}
