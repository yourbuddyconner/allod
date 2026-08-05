//! On-disk graph storage: a `.allod/` directory holding the changeset log,
//! proposals awaiting admission, and demo keys (plain-keypair profile, §6.4.1).
//! Schema lives in the changeset log as meta-typed nodes (no schema directory).
//!
//! Layout:
//!   .allod/graph.yaml                 graph ID, root authority
//!   .allod/keys/<name>.yaml           keypairs (demo: secrets in the clear)
//!   .allod/changesets/<hash>.yaml     admitted changesets
//!   .allod/changesets/<hash>.evidence.yaml  decisions and envelopes
//!   .allod/proposals/<hash>.yaml      pending proposals
//!   .allod/proposals/<hash>.evidence.yaml
//!   .allod/HEAD                       tip changeset hash

use crate::docstore::{DocStore, FsStore};
use crate::fold::State;
use crate::meta::{is_meta_type, meta_registry};
use crate::registry::Registry;
use crate::sign::Keypair;
use crate::{bare, get_str};
use serde_yaml::{Mapping, Value};
use std::path::{Path, PathBuf};

pub struct Graph {
    pub dir: PathBuf,
    store: Box<dyn DocStore>,
    key_backends: Vec<Box<dyn crate::keys::KeyBackend>>,
    /// Index into `key_backends` used by `create_key`.
    ///
    /// Defaults to the first `"file"` backend so that new keys are always written
    /// to the XDG file path unless `key_backends` in graph.yaml explicitly lists
    /// `keychain` first (indicating deliberate opt-in to keychain creation).
    create_backend_idx: usize,
}

/// Return (backends, create_idx) for the default chain on this platform.
///
/// Resolution order on macOS is [keychain, file] so existing keychain keys are
/// found first.  Creation always targets the file backend (index 1 on macOS,
/// index 0 elsewhere) — the keychain is never written by default.
fn default_backends(dir: &Path) -> (Vec<Box<dyn crate::keys::KeyBackend>>, usize) {
    use crate::keys::FileBackend;
    let legacy_keys = dir.join(".allod/keys");
    #[cfg(target_os = "macos")]
    {
        use crate::keys_keychain::KeychainBackend;
        // Resolution: keychain first, then file.
        // Creation: file (index 1) — keychain never written by default.
        return (
            vec![
                Box::new(KeychainBackend::new()),
                Box::new(FileBackend::platform_default(vec![legacy_keys])),
            ],
            1, // file backend
        );
    }
    #[cfg(not(target_os = "macos"))]
    (vec![Box::new(FileBackend::platform_default(vec![legacy_keys]))], 0)
}

fn short(hash: &str) -> &str {
    hash.strip_prefix("sha256:").unwrap_or(hash)
}


/// Speculatively apply the meta-type create/update ops from `cs` to a clone
/// of `state`, then derive and return a `Registry` from the resulting state.
///
/// This gives the effective registry that should govern validation of ALL ops
/// in a changeset that installs new schema alongside other objects — so that
/// the non-meta nodes created in the same changeset can reference types that
/// are also defined in that changeset.
fn speculative_registry_for_changeset(state: &State, cs: &Value) -> Result<Registry, String> {
    use crate::fold::Obj;
    use crate::model::revision_hash;

    let mut speculative = state.clone();
    let meta_bootstrap = meta_registry();

    let Some(ops) = cs.get("operations").and_then(Value::as_sequence) else {
        return Registry::from_state(&speculative);
    };

    for op in ops {
        let Some(map) = op.as_mapping() else { continue };
        let Some((verb, payload)) = map.iter().next() else { continue };
        let Some(verb) = verb.as_str() else { continue };
        if verb != "create" && verb != "update" {
            continue;
        }
        if get_str(payload, "kind") != Some("node") {
            continue;
        }
        let Some(type_ref) = get_str(payload, "type") else { continue };
        if !is_meta_type(type_ref) {
            continue;
        }
        let Some(id) = get_str(payload, "id") else { continue };
        let key = ("node".to_string(), id.to_string());

        // Validate the meta node itself against the meta bootstrap registry.
        // If this fails, the full apply_changeset will also fail and report it.
        // We silently skip invalid meta ops in the speculative pass.
        let content = if verb == "update" {
            let mut c = payload.clone();
            if let Some(m) = c.as_mapping_mut() {
                m.remove("prior");
            }
            c
        } else {
            payload.clone()
        };

        if let Ok(rev) = revision_hash(&content) {
            // Only insert if valid according to meta_bootstrap; skip otherwise.
            let meta_valid = {
                let tref = get_str(&content, "type").unwrap_or("");
                meta_bootstrap.resolve_type(tref, None).is_some()
            };
            if meta_valid {
                speculative.objects.insert(
                    key,
                    Obj { content, rev, deleted: false, redacted: false },
                );
            }
        }
    }

    Registry::from_state(&speculative)
}

/// True if any operation in `cs` creates/updates a node whose payload type
/// is a meta type, or deletes a node that currently has a meta type in `state`.
fn changeset_touches_meta(state: &State, cs: &Value) -> bool {
    let Some(ops) = cs.get("operations").and_then(Value::as_sequence) else {
        return false;
    };
    for op in ops {
        let Some(map) = op.as_mapping() else { continue };
        let Some((verb, payload)) = map.iter().next() else { continue };
        let Some(verb) = verb.as_str() else { continue };
        match verb {
            "create" | "update" if get_str(payload, "kind") == Some("node") => {
                if let Some(t) = get_str(payload, "type") {
                    if is_meta_type(t) {
                        return true;
                    }
                }
            }
            "delete" if get_str(payload, "kind") == Some("node") => {
                // Look up the object's type in state BEFORE applying.
                if let Some(id) = get_str(payload, "id") {
                    if let Some(obj) = state.get_live("node", id) {
                        if let Some(t) = get_str(&obj.content, "type") {
                            if is_meta_type(t) {
                                return true;
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    false
}

impl Graph {
    fn read_yaml(&self, path: &str) -> Result<Value, String> {
        let text = self
            .store
            .read(path)?
            .ok_or_else(|| format!("{path}: not found"))?;
        serde_yaml::from_str(&text).map_err(|e| format!("{path}: {e}"))
    }

    fn write_yaml(&self, path: &str, doc: &Value) -> Result<(), String> {
        let text = serde_yaml::to_string(doc).map_err(|e| e.to_string())?;
        self.store.write(path, &text)
    }

    pub fn create(dir: &Path) -> Result<Graph, String> {
        let store = FsStore::create(dir)?;
        let (key_backends, create_backend_idx) = default_backends(dir);
        Ok(Graph { dir: dir.to_path_buf(), store: Box::new(store), key_backends, create_backend_idx })
    }

    pub fn open(dir: &Path) -> Result<Graph, String> {
        let store = FsStore::open(dir)?;
        let (key_backends, create_backend_idx) = default_backends(dir);
        let mut graph = Graph { dir: dir.to_path_buf(), store: Box::new(store), key_backends, create_backend_idx };
        if graph.store.read("graph.yaml")?.is_none() {
            return Err(format!(
                "{} is not an allod graph (no .allod/graph.yaml)",
                dir.display()
            ));
        }
        // Check for key_backends override in graph.yaml.
        // When the user explicitly lists key_backends, the FIRST entry governs both
        // resolution order and key creation target — this is the opt-in path for
        // keychain creation.  Without an explicit listing the platform defaults apply
        // (file creation even on macOS).
        if let Ok(meta) = graph.meta() {
            if let Some(backends_seq) = meta.get("key_backends").and_then(Value::as_sequence) {
                let mut chain: Vec<Box<dyn crate::keys::KeyBackend>> = Vec::new();
                for item in backends_seq {
                    match item.as_str() {
                        Some("file") => {
                            use crate::keys::FileBackend;
                            let legacy_keys = dir.join(".allod/keys");
                            chain.push(Box::new(FileBackend::platform_default(vec![legacy_keys])));
                        }
                        #[cfg(target_os = "macos")]
                        Some("keychain") => {
                            use crate::keys_keychain::KeychainBackend;
                            chain.push(Box::new(KeychainBackend::new()));
                        }
                        #[cfg(not(target_os = "macos"))]
                        Some("keychain") => {
                            return Err("key backend \"keychain\" is macOS-only".into());
                        }
                        Some(other) => {
                            return Err(format!("unknown key backend {other:?} (not built on this platform)"));
                        }
                        None => return Err("key_backends entries must be strings".into()),
                    }
                }
                // Explicit key_backends list: creation goes to the first entry (index 0).
                graph.create_backend_idx = 0;
                graph.key_backends = chain;
            }
        }
        Ok(graph)
    }

    pub fn with_store(store: Box<dyn DocStore>) -> Graph {
        Graph { dir: PathBuf::new(), store, key_backends: vec![], create_backend_idx: 0 }
    }

    pub fn open_with_store(store: Box<dyn DocStore>) -> Result<Graph, String> {
        if store.read("graph.yaml")?.is_none() {
            return Err("not an allod graph (no .allod/graph.yaml)".into());
        }
        Ok(Graph { dir: PathBuf::new(), store, key_backends: vec![], create_backend_idx: 0 })
    }

    // ---------------- meta ----------------

    pub fn write_meta(&self, graph_id: &str, roots: &[String]) -> Result<(), String> {
        let mut map = Mapping::new();
        map.insert(Value::String("graph_id".into()), Value::String(graph_id.into()));
        map.insert(
            Value::String("root".into()),
            Value::Sequence(roots.iter().cloned().map(Value::String).collect()),
        );
        self.write_yaml("graph.yaml", &Value::Mapping(map))
    }

    pub fn meta(&self) -> Result<Value, String> {
        self.read_yaml("graph.yaml")
    }

    pub fn trusted_measurements(&self) -> Result<Vec<String>, String> {
        Ok(self
            .meta()?
            .get("trusted_measurements")
            .and_then(Value::as_sequence)
            .map(|seq| {
                seq.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Trust a measurement for `simulated` evidence (Appendix A
    /// step 8). A real deployment pins hardware vendor roots instead.
    pub fn trust_measurement(&self, measurement: &str) -> Result<(), String> {
        let mut meta = self.meta()?;
        let map = meta.as_mapping_mut().ok_or("meta must be a map")?;
        let key = Value::String("trusted_measurements".into());
        let mut list = map
            .get(&key)
            .and_then(Value::as_sequence)
            .cloned()
            .unwrap_or_default();
        if !list.iter().any(|m| m.as_str() == Some(measurement)) {
            list.push(Value::String(measurement.into()));
        }
        map.insert(key, Value::Sequence(list));
        self.write_yaml("graph.yaml", &meta)
    }

    // ---------------- checkpoints (§3.2.5) ----------------

    pub fn write_checkpoint(&self, revision: &str, checkpoint: &Value) -> Result<(), String> {
        self.write_yaml(&format!("checkpoints/{}.yaml", short(revision)), checkpoint)
    }

    pub fn checkpoints(&self) -> Result<Vec<Value>, String> {
        let names = self.store.list("checkpoints")?;
        let mut out = Vec::new();
        for name in names {
            out.push(self.read_yaml(&format!("checkpoints/{name}"))?);
        }
        Ok(out)
    }

    pub fn roots(&self) -> Result<Vec<String>, String> {
        Ok(self
            .meta()?
            .get("root")
            .and_then(Value::as_sequence)
            .map(|seq| {
                seq.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default())
    }

    // ---------------- schema ----------------

    pub fn registry(&self) -> Result<Registry, String> {
        // FIXME: fold result could be cached per call site; callers using both fold twice
        let state = self.fold()?;
        Registry::from_state(&state)
    }

    pub fn policy(&self) -> Result<Value, String> {
        // FIXME: fold result could be cached per call site; callers using both fold twice
        let state = self.fold()?;
        // Find the live meta/Policy node and parse its definition attribute.
        for ((kind, _), obj) in &state.objects {
            if kind != "node" || obj.deleted {
                continue;
            }
            let type_ref = match get_str(&obj.content, "type") {
                Some(t) => t,
                None => continue,
            };
            if bare(type_ref) != "meta/Policy" {
                continue;
            }
            let definition = obj
                .content
                .get("attributes")
                .and_then(|a| get_str(a, "definition"))
                .ok_or("meta/Policy node missing definition attribute")?;
            let val: Value = serde_yaml::from_str(definition)
                .map_err(|e| format!("meta/Policy definition is not valid YAML: {e}"))?;
            return Ok(val);
        }
        Err("no policy installed".into())
    }

    // ---------------- keys ----------------

    pub fn save_key(&self, kp: &Keypair) -> Result<(), String> {
        self.write_yaml(&format!("keys/{}.yaml", kp.name), &kp.to_yaml())
    }

    pub fn load_key(&self, name: &str) -> Result<Keypair, String> {
        Keypair::from_yaml(&self.read_yaml(&format!("keys/{name}.yaml"))?)
    }

    pub fn set_key_backends(&mut self, backends: Vec<Box<dyn crate::keys::KeyBackend>>) {
        // When explicitly overriding backends, set creation to the first file backend
        // in the chain (index of first with id == "file"), falling back to 0.
        self.create_backend_idx = backends
            .iter()
            .position(|b| b.id() == "file")
            .unwrap_or(0);
        self.key_backends = backends;
    }

    pub fn signer(&self, name: &str) -> Result<crate::keys::Signer<'_>, String> {
        let graph_id = self.meta()
            .ok()
            .and_then(|m| m.get("graph_id").and_then(Value::as_str).map(String::from))
            .unwrap_or_default();
        for backend in &self.key_backends {
            if let Ok(handle) = backend.resolve(&graph_id, name) {
                return Ok(crate::keys::Signer::from_backend(backend.as_ref(), handle));
            }
        }
        // Fallback: try load_key (in-store .allod/keys/ doc)
        match self.load_key(name) {
            Ok(kp) => Ok(crate::keys::Signer::local(kp)),
            Err(_) => Err(format!("no key for principal {name:?} (tried {} backends + in-store fallback)", self.key_backends.len())),
        }
    }

    pub fn create_key(&self, kp: &Keypair) -> Result<(), String> {
        let graph_id = self.meta()
            .ok()
            .and_then(|m| m.get("graph_id").and_then(Value::as_str).map(String::from))
            .unwrap_or_default();
        // Use create_backend_idx (not first()) so that the default always targets the
        // file backend.  Keychain creation only happens when graph.yaml explicitly lists
        // keychain first (in which case open() sets create_backend_idx = 0).
        if let Some(backend) = self.key_backends.get(self.create_backend_idx) {
            return backend.store_keypair(&graph_id, kp);
        }
        // Empty chain (in-memory graphs): fall back to in-store doc
        self.save_key(kp)
    }

    // ---------------- log ----------------

    pub fn head(&self) -> Result<Option<String>, String> {
        Ok(self
            .store
            .read("HEAD")?
            .map(|s| s.trim().to_string()))
    }

    pub fn append_changeset(
        &self,
        cs: &Value,
        hash: &str,
        evidence: Option<&Value>,
    ) -> Result<(), String> {
        self.write_yaml(&format!("changesets/{}.yaml", short(hash)), cs)?;
        if let Some(evidence) = evidence {
            self.write_yaml(&format!("changesets/{}.evidence.yaml", short(hash)), evidence)?;
        }
        self.store.write("HEAD", hash)
    }

    pub fn read_changeset(&self, hash: &str) -> Result<Value, String> {
        self.read_yaml(&format!("changesets/{}.yaml", short(hash)))
    }

    pub fn read_evidence(&self, hash: &str) -> Result<Option<Value>, String> {
        let path = format!("changesets/{}.evidence.yaml", short(hash));
        match self.store.read(&path)? {
            None => Ok(None),
            Some(text) => {
                let val: Value =
                    serde_yaml::from_str(&text).map_err(|e| format!("{path}: {e}"))?;
                Ok(Some(val))
            }
        }
    }

    pub fn write_evidence(&self, hash: &str, evidence: &Value) -> Result<(), String> {
        self.write_yaml(&format!("changesets/{}.evidence.yaml", short(hash)), evidence)
    }

    /// The chain from genesis to HEAD, in application order.
    pub fn chain(&self) -> Result<Vec<Value>, String> {
        let Some(mut cursor) = self.head()? else {
            return Ok(Vec::new());
        };
        let mut chain = Vec::new();
        loop {
            let cs = self.read_changeset(&cursor)?;
            let parents: Vec<String> = cs
                .get("parents")
                .and_then(Value::as_sequence)
                .map(|seq| {
                    seq.iter()
                        .filter_map(Value::as_str)
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default();
            chain.push(cs);
            match parents.first() {
                Some(parent) => cursor = parent.clone(),
                None => break,
            }
        }
        chain.reverse();
        Ok(chain)
    }

    /// Fold the whole log into a state (§3.2.4).
    ///
    /// The registry is derived incrementally: each changeset is validated
    /// against the registry that would be in effect at that point in
    /// history. When a changeset contains meta-type creates or updates,
    /// those meta ops are pre-applied to a speculative copy of the state
    /// so the full registry (including schema defined within that very
    /// changeset) is available for validating the remaining ops in the
    /// same changeset.
    pub fn fold(&self) -> Result<State, String> {
        self.fold_to(None)
    }

    /// Fold the chain up to and including the changeset whose hash equals
    /// `stop`.  When `stop` is `None`, folds the entire chain (equivalent to
    /// `fold()`).  Returns an error if `stop` names a hash not present in
    /// the chain.
    pub fn fold_to(&self, stop: Option<&str>) -> Result<State, String> {
        let chain = self.chain()?;

        // Derive the registry per-changeset from state.
        //
        // Genesis exception (§4.6 / decision 3): the very first changeset may
        // install schema (meta ops) AND create objects of those new types in
        // the same atomic unit.  For the genesis changeset only, we
        // speculatively pre-apply the meta ops so the registry already knows
        // about types defined within that changeset.  Every subsequent
        // changeset validates strictly against the registry derived from
        // committed state — i.e. types must already be present before the
        // changeset that uses them.
        //
        // Registry cache (§2.5 reuse semantics): Registry::from_state() is
        // O(schema-size) and the schema rarely changes.  We cache the last
        // derived registry and reuse it across consecutive non-meta changesets,
        // invalidating the cache whenever a changeset touches meta nodes (which
        // would change the registry after it applies).
        let mut state = State::default();
        let mut is_genesis = true;
        let mut reg_cache: Option<Registry> = None;
        let mut stop_matched = stop.is_none();
        for cs in &chain {
            let touches_meta = changeset_touches_meta(&state, cs);
            // Invalidate the cache whenever this changeset touches meta nodes —
            // the registry will be different after the changeset applies.
            if touches_meta {
                reg_cache = None;
            }
            let effective_reg = if is_genesis && touches_meta {
                // Genesis only: speculatively apply meta ops to derive the
                // registry that governs validation of ALL ops in this CS.
                speculative_registry_for_changeset(&state, cs)?
            } else if touches_meta {
                // Post-genesis meta changeset: always re-derive from committed
                // state (cache was already invalidated above).
                Registry::from_state(&state)?
            } else {
                // Non-meta changeset: reuse the cached registry if available,
                // otherwise derive and prime the cache.
                if reg_cache.is_none() {
                    reg_cache = Some(Registry::from_state(&state)?);
                }
                // SAFETY: we just ensured reg_cache is Some above.
                reg_cache.as_ref().unwrap().clone()
            };
            is_genesis = false;

            state.apply_changeset(&effective_reg, cs).map_err(|e| {
                format!(
                    "fold rejected changeset {}: {e}",
                    get_str(cs, "hash").unwrap_or("?")
                )
            })?;

            if let Some(target) = stop {
                if get_str(cs, "hash").unwrap_or("") == target {
                    stop_matched = true;
                    break;
                }
            }
        }

        if !stop_matched {
            // stop is Some here (stop.is_none() → stop_matched = true above)
            return Err(format!(
                "revision {} is not in the chain",
                stop.unwrap_or("?")
            ));
        }

        Ok(state)
    }

    // ---------------- proposals ----------------

    pub fn write_proposal(&self, cs: &Value, hash: &str) -> Result<(), String> {
        self.write_yaml(&format!("proposals/{}.yaml", short(hash)), cs)
    }

    pub fn write_proposal_evidence(&self, hash: &str, evidence: &Value) -> Result<(), String> {
        self.write_yaml(&format!("proposals/{}.evidence.yaml", short(hash)), evidence)
    }

    pub fn read_proposal(&self, hash: &str) -> Result<Value, String> {
        self.read_yaml(&format!("proposals/{}.yaml", short(hash)))
    }

    pub fn read_proposal_evidence(&self, hash: &str) -> Result<Value, String> {
        let path = format!("proposals/{}.evidence.yaml", short(hash));
        match self.store.read(&path)? {
            None => {
                let mut map = Mapping::new();
                map.insert(Value::String("decisions".into()), Value::Sequence(vec![]));
                map.insert(Value::String("envelopes".into()), Value::Sequence(vec![]));
                Ok(Value::Mapping(map))
            }
            Some(text) => {
                serde_yaml::from_str(&text).map_err(|e| format!("{path}: {e}"))
            }
        }
    }

    pub fn list_proposals(&self) -> Result<Vec<String>, String> {
        let names = self.store.list("proposals")?;
        let mut hashes = Vec::new();
        for name in names {
            if let Some(stem) = name.strip_suffix(".yaml") {
                if !stem.ends_with(".evidence") {
                    hashes.push(format!("sha256:{stem}"));
                }
            }
        }
        hashes.sort();
        Ok(hashes)
    }

    /// Remove a pending proposal and its evidence file (if any).
    ///
    /// Relies on [`DocStore::remove`] being Ok-on-absent: calling this on a
    /// hash that was never written, or that was already removed, is safe and
    /// returns `Ok(())`.
    pub fn remove_proposal(&self, hash: &str) -> Result<(), String> {
        self.store.remove(&format!("proposals/{}.yaml", short(hash)))?;
        self.store.remove(&format!("proposals/{}.evidence.yaml", short(hash)))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docstore::MemStore;
    use crate::model::changeset_hash;
    use crate::schemaops::compile_schema_ops;

    /// Serialization lock: set_var is process-wide; prevent races with parallel tests.
    static SIGNER_TEST_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();

    #[test]
    fn signer_resolves_xdg_then_legacy_then_store() {
        let _guard = SIGNER_TEST_LOCK.get_or_init(|| std::sync::Mutex::new(())).lock().unwrap();
        // Graph in a temp dir; ALLOD_KEYS_DIR pointed at another temp dir.
        let root = std::env::temp_dir().join(format!(
            "allod-store-signer-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0),
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::env::set_var("ALLOD_KEYS_DIR", root.join("xdg"));
        let gdir = root.join("g");
        let mut graph = Graph::create(&gdir).unwrap();
        graph.write_meta("sha256:cafe", &[]).unwrap();
        // Override the backend chain to use file-only for this test — we are testing
        // the XDG→legacy→store fallback, not the macOS keychain integration.
        // (Keychain integration is tested separately in keys_keychain::tests.)
        {
            use crate::keys::FileBackend;
            let legacy_keys = gdir.join(".allod/keys");
            graph.set_key_backends(vec![
                Box::new(FileBackend::platform_default(vec![legacy_keys])),
            ]);
        }
        // (a) create_key goes to the XDG path, keyed by graph id.
        let kp = Keypair::generate("alice");
        let public = kp.public_hex();
        graph.create_key(&kp).unwrap();
        assert!(root.join("xdg").join("cafe").join("alice.yaml").is_file());
        let s = graph.signer("alice").unwrap();
        assert_eq!(s.public_hex().unwrap(), public);
        // (b) a legacy in-repo key still resolves (fallback read).
        let legacy = Keypair::generate("legacy");
        graph.save_key(&legacy).unwrap(); // store-level write to .allod/keys/
        let s2 = graph.signer("legacy").unwrap();
        assert_eq!(s2.public_hex().unwrap(), legacy.public_hex());
        // (c) unknown principal errors.
        assert!(graph.signer("nobody").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---- helpers for building unsigned test changesets ----

    fn s(v: &str) -> Value {
        Value::String(v.to_string())
    }

    fn mk(pairs: &[(&str, Value)]) -> Value {
        let mut m = serde_yaml::Mapping::new();
        for (k, v) in pairs {
            m.insert(s(k), v.clone());
        }
        Value::Mapping(m)
    }

    /// Build a minimal changeset value (with hash), given a parent hash and ops.
    fn raw_changeset(parent: Option<&str>, ops: Vec<Value>) -> (Value, String) {
        let parents: Vec<Value> = parent.into_iter().map(|p| s(p)).collect();
        let mut cs_map = serde_yaml::Mapping::new();
        cs_map.insert(s("kind"), s("changeset"));
        cs_map.insert(s("parents"), Value::Sequence(parents));
        cs_map.insert(s("operations"), Value::Sequence(ops));
        let cs = Value::Mapping(cs_map);
        let (hash, _, _, _) = changeset_hash(&cs).expect("changeset_hash");
        let mut cs = cs;
        if let Some(m) = cs.as_mapping_mut() {
            m.insert(s("hash"), s(&hash));
        }
        (cs, hash)
    }

    fn create_node_op(id: &str, type_ref: &str, attributes: serde_yaml::Mapping) -> Value {
        mk(&[("create", mk(&[
            ("kind", s("node")),
            ("id", s(id)),
            ("type", s(type_ref)),
            ("attributes", Value::Mapping(attributes)),
        ]))])
    }

    fn memory_docs() -> Vec<(String, Value)> {
        let core_yaml = include_str!("../../../ontologies/core/ontology.yaml");
        let memory_yaml = include_str!("../../../ontologies/memory/ontology.yaml");
        let taxonomy_yaml = include_str!("../../../ontologies/memory/taxonomy.yaml");
        vec![
            ("core".to_string(), serde_yaml::from_str(core_yaml).expect("core YAML")),
            ("memory".to_string(), serde_yaml::from_str(memory_yaml).expect("memory YAML")),
            ("memory-taxonomy".to_string(), serde_yaml::from_str(taxonomy_yaml).expect("taxonomy YAML")),
        ]
    }

    fn memory_policy() -> Value {
        let yaml = include_str!("../../../ontologies/memory/policy-local.yaml");
        serde_yaml::from_str(yaml).expect("policy YAML")
    }

    fn seq_id() -> impl FnMut() -> String {
        let mut n = 0u32;
        move || { n += 1; format!("meta-{n:04}") }
    }

    /// TDD (spec exit criterion 3): incremental registry derived from fold.
    ///
    /// 1. Genesis changeset: compiled memory schema ops + owner User node.
    /// 2. Changeset 2: create a memory/Note.
    /// 3. Changeset 3 (schema): add meta/EntityType for memory/Idea@1.
    /// 4. Changeset 4: create a memory/Idea node.
    ///
    /// Asserts:
    /// - Full fold succeeds.
    /// - Idea-create inserted BEFORE the schema changeset fails fold.
    /// - graph.policy() returns the policy Value.
    /// - graph.registry() resolves memory/Idea after fold.
    #[test]
    fn fold_derives_registry_incrementally() {
        let docs = memory_docs();
        let policy = memory_policy();

        let mut id_gen = seq_id();
        let schema_ops = compile_schema_ops(&docs, Some(&policy), &mut id_gen)
            .expect("compile_schema_ops must succeed");

        // Owner User node op
        let mut user_attrs = serde_yaml::Mapping::new();
        user_attrs.insert(s("display_name"), s("owner"));
        user_attrs.insert(s("keys"), Value::Sequence(vec![]));
        let user_op = create_node_op("user-owner", "core/User@1", user_attrs);

        // Build genesis changeset (schema ops + user node)
        let mut genesis_ops = schema_ops.clone();
        genesis_ops.push(user_op);
        let (cs0, hash0) = raw_changeset(None, genesis_ops);

        // Changeset 2: create memory/Note
        let mut note_attrs = serde_yaml::Mapping::new();
        note_attrs.insert(s("content"), s("hello world"));
        let note_op = create_node_op("note-1", "memory/Note@1", note_attrs);
        let (cs1, hash1) = raw_changeset(Some(&hash0), vec![note_op]);

        // Changeset 3: add meta/EntityType for memory/Idea@1
        let idea_def = "attributes:\n  title: {type: string}\n";
        let mut idea_attrs = serde_yaml::Mapping::new();
        idea_attrs.insert(s("name"), s("Idea"));
        idea_attrs.insert(s("package"), s("memory"));
        idea_attrs.insert(s("definition"), s(idea_def));
        let schema_op = create_node_op("meta-idea-1", "meta/EntityType@1", idea_attrs);
        let (cs2, hash2) = raw_changeset(Some(&hash1), vec![schema_op]);

        // Changeset 4: create memory/Idea node
        let mut idea_node_attrs = serde_yaml::Mapping::new();
        idea_node_attrs.insert(s("title"), s("first idea"));
        let idea_node_op = create_node_op("idea-1", "memory/Idea@1", idea_node_attrs);
        let (cs3, _hash3) = raw_changeset(Some(&hash2), vec![idea_node_op.clone()]);

        // Set up the graph with all 4 changesets in order
        let graph = Graph::with_store(Box::new(MemStore::new()));
        graph.write_meta("test-graph", &[]).unwrap();
        graph.append_changeset(&cs0, &hash0, None).unwrap();
        graph.append_changeset(&cs1, &hash1, None).unwrap();
        graph.append_changeset(&cs2, &hash2, None).unwrap();
        graph.append_changeset(&cs3, &_hash3, None).unwrap();

        // Assert: full fold succeeds
        let state = graph.fold().expect("full fold must succeed");
        assert!(state.get_live("node", "idea-1").is_some(), "idea-1 must be live after fold");

        // Assert: graph.registry() resolves memory/Idea
        let reg = graph.registry().expect("registry must succeed");
        assert!(
            reg.resolve_type("memory/Idea", None).is_some(),
            "registry must resolve memory/Idea after fold"
        );

        // Assert: graph.policy() returns the policy Value
        let pol = graph.policy().expect("policy must succeed");
        assert!(
            pol.get("policy").is_some() || pol.get("default_posture").is_some(),
            "policy must have policy content, got: {pol:?}"
        );

        // Assert: Idea-create BEFORE the schema changeset fails fold.
        // Build a variant with cs2 and cs3 swapped: idea-create before schema.
        let graph_bad = Graph::with_store(Box::new(MemStore::new()));
        graph_bad.write_meta("test-graph-bad", &[]).unwrap();
        graph_bad.append_changeset(&cs0, &hash0, None).unwrap();
        graph_bad.append_changeset(&cs1, &hash1, None).unwrap();
        // Insert the idea-create before the schema changeset
        let (cs_bad_idea, bad_idea_hash) = raw_changeset(Some(&hash1), vec![idea_node_op]);
        let (cs_bad_schema, bad_schema_hash) = raw_changeset(Some(&bad_idea_hash), vec![{
            let mut idea_attrs2 = serde_yaml::Mapping::new();
            idea_attrs2.insert(s("name"), s("Idea"));
            idea_attrs2.insert(s("package"), s("memory"));
            idea_attrs2.insert(s("definition"), s(idea_def));
            create_node_op("meta-idea-2", "meta/EntityType@1", idea_attrs2)
        }]);
        graph_bad.append_changeset(&cs_bad_idea, &bad_idea_hash, None).unwrap();
        graph_bad.append_changeset(&cs_bad_schema, &bad_schema_hash, None).unwrap();
        match graph_bad.fold() {
            Ok(_) => panic!("fold must fail when Idea-create appears before schema changeset"),
            Err(err) => {
                assert!(
                    err.contains("memory/Idea") || err.contains("does not resolve") || err.contains("reject"),
                    "error must mention schema resolution failure, got: {err}"
                );
            }
        }
    }

    /// Regression guard (§4.6 / decision 3): a POST-genesis changeset that
    /// combines a `meta/EntityType` create for `memory/Idea@1` with a
    /// `memory/Idea` node create in the SAME changeset must FAIL fold.
    ///
    /// The Idea type is not present in the parent-revision registry, so the
    /// speculative pre-apply is NOT allowed outside of genesis.  This test
    /// verifies that re-broadening the genesis exception is caught immediately.
    #[test]
    fn post_genesis_same_changeset_smuggle_fails() {
        let docs = memory_docs();
        let policy = memory_policy();

        let mut id_gen = seq_id();
        let schema_ops = compile_schema_ops(&docs, Some(&policy), &mut id_gen)
            .expect("compile_schema_ops must succeed");

        // Genesis: schema ops only (no user node, keeps it minimal)
        let (cs0, hash0) = raw_changeset(None, schema_ops);

        // Post-genesis: ONE changeset that BOTH defines memory/Idea@1 schema
        // AND creates a memory/Idea node — this is the "smuggle" pattern.
        let idea_def = "attributes:\n  title: {type: string}\n";
        let mut idea_attrs = serde_yaml::Mapping::new();
        idea_attrs.insert(s("name"), s("Idea"));
        idea_attrs.insert(s("package"), s("memory"));
        idea_attrs.insert(s("definition"), s(idea_def));
        let schema_op = create_node_op("meta-idea-1", "meta/EntityType@1", idea_attrs);

        let mut idea_node_attrs = serde_yaml::Mapping::new();
        idea_node_attrs.insert(s("title"), s("first idea"));
        let idea_node_op = create_node_op("idea-1", "memory/Idea@1", idea_node_attrs);

        // Both ops in a single post-genesis changeset.
        let (cs_smuggle, smuggle_hash) =
            raw_changeset(Some(&hash0), vec![schema_op, idea_node_op]);

        let graph = Graph::with_store(Box::new(MemStore::new()));
        graph.write_meta("test-smuggle", &[]).unwrap();
        graph.append_changeset(&cs0, &hash0, None).unwrap();
        graph.append_changeset(&cs_smuggle, &smuggle_hash, None).unwrap();

        match graph.fold() {
            Ok(_) => panic!(
                "fold must reject a post-genesis changeset that smuggles schema + instance together"
            ),
            Err(err) => {
                assert!(
                    err.contains("memory/Idea") || err.contains("does not resolve") || err.contains("reject"),
                    "error must mention schema resolution failure, got: {err}"
                );
            }
        }
    }

    /// Registry cache correctness: fold must succeed (and produce the right
    /// live nodes) when a genesis schema changeset is followed by several
    /// consecutive non-meta changesets that exercise the `reg_cache` fast path.
    #[test]
    fn fold_caches_registry_for_non_meta_chain() {
        // Genesis: schema ops with policy (so the registry is non-trivial),
        // plus a User node so the graph is fully valid.
        let docs = memory_docs();
        let policy = memory_policy();
        let mut id_gen = seq_id();
        let schema_ops = compile_schema_ops(&docs, Some(&policy), &mut id_gen)
            .expect("compile_schema_ops");

        let mut user_attrs = serde_yaml::Mapping::new();
        user_attrs.insert(s("display_name"), s("owner"));
        user_attrs.insert(s("keys"), Value::Sequence(vec![]));
        let user_op = create_node_op("user-owner", "core/User@1", user_attrs);
        let mut genesis_ops = schema_ops;
        genesis_ops.push(user_op);
        let (cs0, hash0) = raw_changeset(None, genesis_ops);

        // Three non-meta changesets: each creates a memory/Note node.
        // These exercise the reg_cache path: the registry is derived once
        // after genesis and then reused for each subsequent changeset.
        let mut parent = hash0.clone();
        let mut hashes = vec![hash0.clone()];
        let mut changesets = vec![cs0];
        for i in 1..=3 {
            let mut note_attrs = serde_yaml::Mapping::new();
            note_attrs.insert(s("content"), s(&format!("note {i}")));
            let note_op = create_node_op(&format!("note-{i}"), "memory/Note@1", note_attrs);
            let (cs, hash) = raw_changeset(Some(&parent), vec![note_op]);
            parent = hash.clone();
            hashes.push(hash);
            changesets.push(cs);
        }

        let graph = Graph::with_store(Box::new(MemStore::new()));
        graph.write_meta("test-cache", &[]).unwrap();
        for (cs, hash) in changesets.iter().zip(hashes.iter()) {
            graph.append_changeset(cs, hash, None).unwrap();
        }

        // Full fold must succeed and all 3 notes must be live.
        let state = graph.fold().expect("fold must succeed with reg cache");
        for i in 1..=3 {
            assert!(
                state.get_live("node", &format!("note-{i}")).is_some(),
                "note-{i} must be live after fold"
            );
        }
    }

    #[test]
    fn graph_over_memstore() {
        let graph = Graph::with_store(Box::new(MemStore::new()));
        graph.write_meta("sha256:genesis", &["principal:o".into()]).unwrap();
        assert_eq!(graph.roots().unwrap(), vec!["principal:o".to_string()]);
        assert_eq!(graph.head().unwrap(), None);
        let cs: Value = serde_yaml::from_str("hash: sha256:ab\nparents: []").unwrap();
        graph.append_changeset(&cs, "sha256:ab", None).unwrap();
        assert_eq!(graph.head().unwrap().as_deref(), Some("sha256:ab"));
        assert!(graph.read_evidence("sha256:ab").unwrap().is_none());
        graph.write_evidence("sha256:ab", &cs).unwrap();
        assert!(graph.read_evidence("sha256:ab").unwrap().is_some());
    }

    /// TDD: fold_to(Some(hash)) stops after applying the named changeset.
    #[test]
    fn fold_to_stops_at_the_named_revision() {
        // Arrange: genesis schema + 2 non-meta changesets (cs1 creates node a, cs2 creates node b)
        let docs = memory_docs();
        let policy = memory_policy();
        let mut id_gen = seq_id();
        let schema_ops = compile_schema_ops(&docs, Some(&policy), &mut id_gen)
            .expect("compile_schema_ops");

        let mut user_attrs = serde_yaml::Mapping::new();
        user_attrs.insert(s("display_name"), s("owner"));
        user_attrs.insert(s("keys"), Value::Sequence(vec![]));
        let user_op = create_node_op("user-owner", "core/User@1", user_attrs);
        let mut genesis_ops = schema_ops;
        genesis_ops.push(user_op);
        let (cs0, hash0) = raw_changeset(None, genesis_ops);

        // cs1: creates node a (memory/Note with id "a")
        let mut attrs_a = serde_yaml::Mapping::new();
        attrs_a.insert(s("content"), s("node a"));
        let op_a = create_node_op("a", "memory/Note@1", attrs_a);
        let (cs1, cs1_hash) = raw_changeset(Some(&hash0), vec![op_a]);

        // cs2: creates node b (memory/Note with id "b")
        let mut attrs_b = serde_yaml::Mapping::new();
        attrs_b.insert(s("content"), s("node b"));
        let op_b = create_node_op("b", "memory/Note@1", attrs_b);
        let (cs2, cs2_hash) = raw_changeset(Some(&cs1_hash), vec![op_b]);

        let graph = Graph::with_store(Box::new(MemStore::new()));
        graph.write_meta("test-fold-to", &[]).unwrap();
        graph.append_changeset(&cs0, &hash0, None).unwrap();
        graph.append_changeset(&cs1, &cs1_hash, None).unwrap();
        graph.append_changeset(&cs2, &cs2_hash, None).unwrap();

        // Assert:
        let full = graph.fold_to(None).unwrap();
        let at_cs1 = graph.fold_to(Some(&cs1_hash)).unwrap();
        assert!(full.get_live("node", "b").is_some());
        assert!(at_cs1.get_live("node", "b").is_none(), "cs2 must not be applied");
        assert!(at_cs1.get_live("node", "a").is_some());
        assert_eq!(
            graph.fold().unwrap().state_hash().unwrap(),
            full.state_hash().unwrap(),
            "fold() must equal fold_to(None)"
        );
        let err = match graph.fold_to(Some("sha256:nope")) {
            Err(e) => e,
            Ok(_) => panic!("fold_to unknown hash must fail"),
        };
        assert!(err.contains("not in the chain"), "got: {err}");
    }

    #[test]
    fn remove_proposal_absent_is_ok() {
        let graph = Graph::with_store(Box::new(MemStore::new()));

        // Removing a hash that was never written must not error.
        assert!(graph.remove_proposal("sha256:deadbeef").is_ok());

        // Write a proposal (no evidence).
        let cs: Value = serde_yaml::from_str("hash: sha256:aa
parents: []").unwrap();
        graph.write_proposal(&cs, "sha256:aa").unwrap();
        let proposals = graph.list_proposals().unwrap();
        assert!(proposals.contains(&"sha256:aa".to_string()), "proposal should be listed");

        // Remove it — should succeed.
        assert!(graph.remove_proposal("sha256:aa").is_ok());
        let proposals = graph.list_proposals().unwrap();
        assert!(!proposals.contains(&"sha256:aa".to_string()), "proposal should be gone");

        // Remove again (now absent) — still Ok.
        assert!(graph.remove_proposal("sha256:aa").is_ok());
    }
}
