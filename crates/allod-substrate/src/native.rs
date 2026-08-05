//! `NativeSubstrate` (§3.2): the native changeset log presented
//! through the §3.1 interface. Pure adapter — no new semantics; every
//! answer comes from `allod_core::store::Graph`.

use crate::{AuthorVerdict, Revision, Substrate};
use allod_core::get_str;
use allod_core::model::changeset_hash;
use allod_core::sign;
use allod_core::store::Graph;
use serde_yaml::Value;

pub struct NativeSubstrate<'g> {
    graph: &'g Graph,
}

impl<'g> NativeSubstrate<'g> {
    pub fn new(graph: &'g Graph) -> NativeSubstrate<'g> {
        NativeSubstrate { graph }
    }

    /// Read a changeset and enforce content addressing (§3.1 property 1):
    /// the stored bytes must recompute to the requested hash.
    fn read_checked(&self, hash: &str) -> Result<Value, String> {
        let cs = self.graph.read_changeset(hash)?;
        let (computed, _, _, _) = changeset_hash(&cs)?;
        if computed != hash {
            return Err(format!(
                "revision identity mismatch: {hash} stored, {computed} recomputed"
            ));
        }
        Ok(cs)
    }
}

impl Substrate for NativeSubstrate<'_> {
    fn revision(&self, hash: &str) -> Result<Revision, String> {
        let cs = self.read_checked(hash)?;
        let parents = cs
            .get("parents")
            .and_then(Value::as_sequence)
            .map(|s| s.iter().filter_map(Value::as_str).map(String::from).collect())
            .unwrap_or_default();
        Ok(Revision {
            hash: hash.to_string(),
            parents,
            author: cs.get("author").cloned().unwrap_or(Value::Null),
            timestamp: get_str(&cs, "timestamp").map(String::from),
            signed: cs.get("signature").is_some(),
        })
    }

    fn operation_set(&self, hash: &str) -> Result<Vec<Value>, String> {
        let cs = self.read_checked(hash)?;
        Ok(cs
            .get("operations")
            .and_then(Value::as_sequence)
            .cloned()
            .unwrap_or_default())
    }

    fn state_hash(&self, hash: &str) -> Result<String, String> {
        self.graph.fold_to(Some(hash))?.state_hash()
    }

    fn heads(&self) -> Result<Vec<(String, String)>, String> {
        Ok(self.graph.head()?.map(|h| vec![("HEAD".to_string(), h)]).unwrap_or_default())
    }

    fn verify_authorship(&self, hash: &str) -> Result<AuthorVerdict, String> {
        let cs = self.read_checked(hash)?;
        let Some(signature) = get_str(&cs, "signature").map(String::from) else {
            return Ok(AuthorVerdict::Unsigned);
        };
        let author = cs.get("author").cloned().unwrap_or(Value::Null);
        let principal = get_str(&author, "principal").unwrap_or("").to_string();
        let key_id = get_str(&author, "key").unwrap_or("").to_string();

        // Key lookup state: the revision's first parent, matching
        // flows::verify (line 961 of flows.rs). Genesis self-registers
        // its author, so fall back to the state at the revision itself.
        //
        // Note: flows::verify calls sign::verify(&public, &hash, &signature)
        // where &hash is the bare hash string — not a different preimage.
        // This adapter matches that exactly.
        let rev = self.revision(hash)?;
        let state = match rev.parents.first() {
            Some(parent) => self.graph.fold_to(Some(parent))?,
            None => self.graph.fold_to(Some(hash))?,
        };
        let state = if state.public_key_of(&principal, &key_id).is_some() {
            state
        } else {
            self.graph.fold_to(Some(hash))?
        };
        let Some(public) = state.public_key_of(&principal, &key_id) else {
            return Ok(AuthorVerdict::Failed(format!(
                "no active key {key_id} for {principal}"
            )));
        };
        match sign::verify(&public, hash, &signature) {
            Ok(()) => Ok(AuthorVerdict::Verified { principal, key_id }),
            Err(e) => Ok(AuthorVerdict::Failed(e)),
        }
    }
}
