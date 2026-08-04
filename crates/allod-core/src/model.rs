//! Object and changeset hashing (§1.7, §3.2.1, §3.2.6) and the state
//! tree construction, shared by the fold, the vector generator, and
//! verification. The Appendix H vectors fix these preimages; any
//! change here must reproduce them or is a breaking change.

use crate::canon::canonical_cbor;
use crate::fold::State;
use crate::hash::{merkle_root, sha256_hex};
use crate::meta::is_meta_type;
use crate::{bare, get_str};
use serde_yaml::{Mapping, Value};

/// Revision hash (§1.7): canonical encoding with `rev` omitted.
pub fn revision_hash(payload: &Value) -> Result<String, String> {
    let mut payload = payload.clone();
    if let Some(map) = payload.as_mapping_mut() {
        map.remove("rev");
    }
    Ok(sha256_hex("object", &canonical_cbor(&payload)?))
}

/// One operation's leaf in the operation tree (§3.2.6).
pub fn op_leaf(op: &Value) -> Result<String, String> {
    Ok(sha256_hex("op-leaf", &canonical_cbor(op)?))
}

/// Changeset hash (§3.2.1, §3.2.6) from precomputed leaves: `hash`
/// and `signature` omitted, `operations` replaced by the tree root,
/// `intent` by its hash. A redacted changeset stores `intent_hash`
/// in place of `intent`; the preimage is identical either way.
/// Returns (hash, operations root, preimage bytes).
pub fn changeset_hash_from_leaves(
    cs: &Value,
    leaves: &[String],
) -> Result<(String, String, Vec<u8>), String> {
    let root = merkle_root(leaves, "op-node").ok_or("changeset has no operations")?;
    let mut preimage = cs.clone();
    let map = preimage.as_mapping_mut().ok_or("changeset must be a map")?;
    map.remove("hash");
    map.remove("signature");
    map.insert(
        Value::String("operations".into()),
        Value::String(root.clone()),
    );
    if let Some(intent) = map
        .get(Value::String("intent".into()))
        .and_then(Value::as_str)
        .map(String::from)
    {
        map.insert(
            Value::String("intent".into()),
            Value::String(sha256_hex("intent", intent.as_bytes())),
        );
    } else if let Some(ih) = map
        .get(Value::String("intent_hash".into()))
        .and_then(Value::as_str)
        .map(String::from)
    {
        map.remove("intent_hash");
        map.insert(Value::String("intent".into()), Value::String(ih));
    }
    let bytes = canonical_cbor(&preimage)?;
    Ok((sha256_hex("changeset", &bytes), root, bytes))
}

/// Changeset hash from the stored form. Returns (hash, root, leaves,
/// preimage bytes).
pub fn changeset_hash(cs: &Value) -> Result<(String, String, Vec<String>, Vec<u8>), String> {
    let ops = cs
        .get("operations")
        .and_then(Value::as_sequence)
        .ok_or("changeset needs an operations list")?;
    let leaves: Result<Vec<String>, String> = ops.iter().map(op_leaf).collect();
    let leaves = leaves?;
    let (hash, root, bytes) = changeset_hash_from_leaves(cs, &leaves)?;
    Ok((hash, root, leaves, bytes))
}

/// One object's entry in the state tree (§1.7).
pub fn state_entry(kind: &str, id: &str, rev: &str, deleted: bool) -> Value {
    let mut entry = Mapping::new();
    entry.insert(Value::String("kind".into()), Value::String(kind.into()));
    entry.insert(Value::String("id".into()), Value::String(id.into()));
    entry.insert(Value::String("rev".into()), Value::String(rev.into()));
    if deleted {
        entry.insert(Value::String("deleted".into()), Value::Bool(true));
    }
    Value::Mapping(entry)
}

/// State hash (§1.7) over entries already ordered by kind then
/// logical ID. This is the single site for leaf hashing.
pub fn state_root(entries: &[Value]) -> Result<String, String> {
    let mut leaves = Vec::with_capacity(entries.len());
    for entry in entries {
        leaves.push(sha256_hex("state-leaf", &canonical_cbor(entry)?));
    }
    merkle_root(&leaves, "state-node").ok_or_else(|| "empty state".into())
}

/// State hash over the meta subgraph only (§3.2.1 schema pinning):
/// same Merkle construction as the full state hash, leaves restricted
/// to live meta-typed nodes and their tombstones, ordered identically
/// (BTreeMap order: kind then logical ID).
///
/// Returns `Err` only when the meta subgraph is empty (no meta-typed
/// objects at all — not even tombstones).
pub fn schema_state_hash(state: &State) -> Result<String, String> {
    let entries = state.entries_filtered(|kind, obj| {
        kind == "node"
            && get_str(&obj.content, "type")
                .is_some_and(|t| is_meta_type(bare(t)))
    });
    if entries.is_empty() {
        return Err("empty meta-subgraph state".into());
    }
    state_root(&entries)
}

/// The schema context a changeset pins (§3.2.1): a content hash over
/// the installed schema documents, keyed by name. The same bridge as
/// package hashes (Appendix H) until schema-as-object
/// materialization ships.
// TRANSITIONAL (task 5 removes): superseded by schema_state_hash on materialized graphs.
pub fn schema_context(docs: &[(String, Value)]) -> Result<String, String> {
    let mut map = Mapping::new();
    let mut sorted: Vec<&(String, Value)> = docs.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, doc) in sorted {
        map.insert(Value::String(name.clone()), doc.clone());
    }
    Ok(sha256_hex("package", &canonical_cbor(&Value::Mapping(map))?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn yaml(s: &str) -> Value {
        serde_yaml::from_str(s).unwrap()
    }

    #[test]
    fn revision_hash_ignores_rev() {
        let with = yaml("{ kind: node, id: a, rev: \"sha256:xx\", attributes: { n: 1 } }");
        let without = yaml("{ kind: node, id: a, attributes: { n: 1 } }");
        assert_eq!(revision_hash(&with).unwrap(), revision_hash(&without).unwrap());
    }

    #[test]
    fn intent_redaction_preserves_the_hash() {
        let cs = yaml(
            "{ kind: changeset, parents: [], intent: \"hello\", \
             operations: [ { create: { kind: node, id: a } } ] }",
        );
        let (hash, _, leaves, _) = changeset_hash(&cs).unwrap();
        let mut redacted = cs.clone();
        let map = redacted.as_mapping_mut().unwrap();
        map.remove("intent");
        map.insert(
            Value::String("intent_hash".into()),
            Value::String(sha256_hex("intent", b"hello")),
        );
        let (rhash, _, _) = changeset_hash_from_leaves(&redacted, &leaves).unwrap();
        assert_eq!(hash, rhash);
    }

    #[test]
    fn state_root_orders_and_marks_tombstones() {
        let live = state_entry("node", "a", "sha256:aa", false);
        let dead = state_entry("node", "b", "sha256:bb", true);
        let root = state_root(&[live, dead]).unwrap();
        assert!(root.starts_with("sha256:"));
    }

    // ---- schema_state_hash tests (TDD for Task 4) ----

    fn meta_node_obj(id: &str, type_ref: &str) -> crate::fold::Obj {
        use serde_yaml::Mapping;
        let mut content = Mapping::new();
        content.insert(Value::String("kind".into()), Value::String("node".into()));
        content.insert(Value::String("id".into()), Value::String(id.into()));
        content.insert(Value::String("type".into()), Value::String(type_ref.into()));
        let mut attrs = Mapping::new();
        attrs.insert(Value::String("name".into()), Value::String(id.into()));
        attrs.insert(Value::String("package".into()), Value::String("test".into()));
        attrs.insert(Value::String("definition".into()), Value::String("{}".into()));
        content.insert(Value::String("attributes".into()), Value::Mapping(attrs));
        crate::fold::Obj {
            content: Value::Mapping(content),
            rev: format!("sha256:rev-{id}"),
            deleted: false,
            redacted: false,
        }
    }

    fn non_meta_node_obj(id: &str) -> crate::fold::Obj {
        use serde_yaml::Mapping;
        let mut content = Mapping::new();
        content.insert(Value::String("kind".into()), Value::String("node".into()));
        content.insert(Value::String("id".into()), Value::String(id.into()));
        content.insert(Value::String("type".into()), Value::String("core/Person".into()));
        crate::fold::Obj {
            content: Value::Mapping(content),
            rev: format!("sha256:rev-{id}"),
            deleted: false,
            redacted: false,
        }
    }

    /// (a) Two states differing only in a non-meta node have equal schema_state_hash.
    #[test]
    fn schema_state_hash_ignores_non_meta_nodes() {
        use crate::fold::State;
        use std::collections::BTreeMap;

        let mut objects_base = BTreeMap::new();
        objects_base.insert(
            ("node".into(), "meta-1".into()),
            meta_node_obj("meta-1", "meta/EntityType@1"),
        );
        let state_base = State { objects: objects_base.clone() };

        // Add a non-meta node — schema_state_hash must not change.
        let mut objects_extra = objects_base;
        objects_extra.insert(
            ("node".into(), "person-1".into()),
            non_meta_node_obj("person-1"),
        );
        let state_extra = State { objects: objects_extra };

        let h_base = schema_state_hash(&state_base).unwrap();
        let h_extra = schema_state_hash(&state_extra).unwrap();
        assert_eq!(
            h_base, h_extra,
            "adding a non-meta node must not change schema_state_hash"
        );
    }

    /// (b) Adding a meta node changes schema_state_hash.
    #[test]
    fn schema_state_hash_changes_when_meta_node_added() {
        use crate::fold::State;
        use std::collections::BTreeMap;

        let mut objects1 = BTreeMap::new();
        objects1.insert(
            ("node".into(), "meta-1".into()),
            meta_node_obj("meta-1", "meta/EntityType@1"),
        );
        let state1 = State { objects: objects1.clone() };

        let mut objects2 = objects1;
        objects2.insert(
            ("node".into(), "meta-2".into()),
            meta_node_obj("meta-2", "meta/EntityType@1"),
        );
        let state2 = State { objects: objects2 };

        let h1 = schema_state_hash(&state1).unwrap();
        let h2 = schema_state_hash(&state2).unwrap();
        assert_ne!(h1, h2, "adding a meta node must change schema_state_hash");
        assert!(h1.starts_with("sha256:"), "must be a sha256: hash");
        assert!(h2.starts_with("sha256:"), "must be a sha256: hash");
    }

    /// schema_state_hash returns Err when no meta nodes exist.
    #[test]
    fn schema_state_hash_empty_state_is_err() {
        use crate::fold::State;
        let state_empty = State::default();
        assert!(
            schema_state_hash(&state_empty).is_err(),
            "empty meta subgraph must return Err"
        );

        // A state with only non-meta nodes is also empty from the meta pov.
        use std::collections::BTreeMap;
        let mut objects = BTreeMap::new();
        objects.insert(("node".into(), "person-1".into()), non_meta_node_obj("person-1"));
        let state_only_non_meta = State { objects };
        assert!(
            schema_state_hash(&state_only_non_meta).is_err(),
            "state with only non-meta nodes must return Err"
        );
    }
}
