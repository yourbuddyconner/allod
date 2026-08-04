//! On-disk graph storage: a `.allod/` directory holding the schema
//! projection, the changeset log, proposals awaiting admission, and
//! demo keys (plain-keypair profile, §6.4.1).
//!
//! Layout:
//!   .allod/graph.yaml                 graph ID, root authority
//!   .allod/schema/<name>.yaml         installed schema documents
//!   .allod/keys/<name>.yaml           keypairs (demo: secrets in the clear)
//!   .allod/changesets/<hash>.yaml     admitted changesets
//!   .allod/changesets/<hash>.evidence.yaml  decisions and envelopes
//!   .allod/proposals/<hash>.yaml      pending proposals
//!   .allod/proposals/<hash>.evidence.yaml
//!   .allod/HEAD                       tip changeset hash

use crate::docstore::{DocStore, FsStore};
use crate::fold::State;
use crate::registry::Registry;
use crate::sign::Keypair;
use crate::{get_str, Loaded};
use serde_yaml::{Mapping, Value};
use std::path::{Path, PathBuf};

pub struct Graph {
    pub dir: PathBuf,
    store: Box<dyn DocStore>,
}

fn short(hash: &str) -> &str {
    hash.strip_prefix("sha256:").unwrap_or(hash)
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
        Ok(Graph { dir: dir.to_path_buf(), store: Box::new(store) })
    }

    pub fn open(dir: &Path) -> Result<Graph, String> {
        let store = FsStore::open(dir)?;
        let graph = Graph { dir: dir.to_path_buf(), store: Box::new(store) };
        if graph.store.read("graph.yaml")?.is_none() {
            return Err(format!(
                "{} is not an allod graph (no .allod/graph.yaml)",
                dir.display()
            ));
        }
        Ok(graph)
    }

    pub fn with_store(store: Box<dyn DocStore>) -> Graph {
        Graph { dir: PathBuf::new(), store }
    }

    pub fn open_with_store(store: Box<dyn DocStore>) -> Result<Graph, String> {
        if store.read("graph.yaml")?.is_none() {
            return Err("not an allod graph (no .allod/graph.yaml)".into());
        }
        Ok(Graph { dir: PathBuf::new(), store })
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

    pub fn install_schema(&self, name: &str, doc: &Value) -> Result<(), String> {
        self.write_yaml(&format!("schema/{name}.yaml"), doc)
    }

    pub fn schema_docs(&self) -> Result<Vec<(String, Value)>, String> {
        let names = self.store.list("schema")?;
        let mut docs = Vec::new();
        for name in names {
            if !name.ends_with(".yaml") {
                continue;
            }
            let stem = name.strip_suffix(".yaml").unwrap_or(&name).to_string();
            let doc = self.read_yaml(&format!("schema/{name}"))?;
            docs.push((stem, doc));
        }
        Ok(docs)
    }

    pub fn registry(&self) -> Result<Registry, String> {
        let docs = self.schema_docs()?;
        let Loaded { registry, issues, .. } = crate::loader::load_docs(&docs);
        if let Some(issue) = issues.first() {
            return Err(format!("schema load: {}: {}", issue.context, issue.message));
        }
        Ok(registry)
    }

    pub fn policy(&self) -> Result<Value, String> {
        for (_, doc) in self.schema_docs()? {
            if doc.get("policy").is_some() {
                return Ok(doc);
            }
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
    pub fn fold(&self) -> Result<State, String> {
        let reg = self.registry()?;
        let mut state = State::default();
        for cs in self.chain()? {
            state.apply_changeset(&reg, &cs).map_err(|e| {
                format!(
                    "fold rejected changeset {}: {e}",
                    get_str(&cs, "hash").unwrap_or("?")
                )
            })?;
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
