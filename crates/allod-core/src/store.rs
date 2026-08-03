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

use crate::fold::State;
use crate::registry::Registry;
use crate::sign::Keypair;
use crate::{get_str, Loaded};
use serde_yaml::{Mapping, Value};
use std::fs;
use std::path::{Path, PathBuf};

pub struct Graph {
    pub dir: PathBuf,
}

fn read_yaml(path: &Path) -> Result<Value, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_yaml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn write_yaml(path: &Path, doc: &Value) -> Result<(), String> {
    let text = serde_yaml::to_string(doc).map_err(|e| e.to_string())?;
    fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

fn short(hash: &str) -> &str {
    hash.strip_prefix("sha256:").unwrap_or(hash)
}

impl Graph {
    fn allod(&self) -> PathBuf {
        self.dir.join(".allod")
    }

    pub fn create(dir: &Path) -> Result<Graph, String> {
        let graph = Graph { dir: dir.to_path_buf() };
        for sub in ["schema", "keys", "changesets", "proposals"] {
            fs::create_dir_all(graph.allod().join(sub)).map_err(|e| e.to_string())?;
        }
        Ok(graph)
    }

    pub fn open(dir: &Path) -> Result<Graph, String> {
        let graph = Graph { dir: dir.to_path_buf() };
        if !graph.allod().join("graph.yaml").exists() {
            return Err(format!("{} is not an allod graph (no .allod/graph.yaml)", dir.display()));
        }
        Ok(graph)
    }

    // ---------------- meta ----------------

    pub fn write_meta(&self, graph_id: &str, roots: &[String]) -> Result<(), String> {
        let mut map = Mapping::new();
        map.insert(Value::String("graph_id".into()), Value::String(graph_id.into()));
        map.insert(
            Value::String("root".into()),
            Value::Sequence(roots.iter().cloned().map(Value::String).collect()),
        );
        write_yaml(&self.allod().join("graph.yaml"), &Value::Mapping(map))
    }

    pub fn meta(&self) -> Result<Value, String> {
        read_yaml(&self.allod().join("graph.yaml"))
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
        write_yaml(&self.allod().join("graph.yaml"), &meta)
    }

    // ---------------- checkpoints (§3.2.5) ----------------

    pub fn write_checkpoint(&self, revision: &str, checkpoint: &Value) -> Result<(), String> {
        let dir = self.allod().join("checkpoints");
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        write_yaml(&dir.join(format!("{}.yaml", short(revision))), checkpoint)
    }

    pub fn checkpoints(&self) -> Result<Vec<Value>, String> {
        let dir = self.allod().join("checkpoints");
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        let mut paths: Vec<PathBuf> = fs::read_dir(&dir)
            .map_err(|e| e.to_string())?
            .flatten()
            .map(|e| e.path())
            .collect();
        paths.sort();
        for path in paths {
            out.push(read_yaml(&path)?);
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
        write_yaml(&self.allod().join("schema").join(format!("{name}.yaml")), doc)
    }

    pub fn schema_docs(&self) -> Result<Vec<(String, Value)>, String> {
        let dir = self.allod().join("schema");
        let mut docs = Vec::new();
        let mut paths: Vec<PathBuf> = fs::read_dir(&dir)
            .map_err(|e| e.to_string())?
            .flatten()
            .map(|e| e.path())
            .collect();
        paths.sort();
        for path in paths {
            if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
                continue;
            }
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("schema")
                .to_string();
            docs.push((name, read_yaml(&path)?));
        }
        Ok(docs)
    }

    pub fn registry(&self) -> Result<Registry, String> {
        let Loaded { registry, issues, .. } =
            crate::load_dir(&self.allod().join("schema"));
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
        write_yaml(
            &self.allod().join("keys").join(format!("{}.yaml", kp.name)),
            &kp.to_yaml(),
        )
    }

    pub fn load_key(&self, name: &str) -> Result<Keypair, String> {
        Keypair::from_yaml(&read_yaml(
            &self.allod().join("keys").join(format!("{name}.yaml")),
        )?)
    }

    // ---------------- log ----------------

    pub fn head(&self) -> Result<Option<String>, String> {
        let path = self.allod().join("HEAD");
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(
            fs::read_to_string(&path)
                .map_err(|e| e.to_string())?
                .trim()
                .to_string(),
        ))
    }

    pub fn append_changeset(
        &self,
        cs: &Value,
        hash: &str,
        evidence: Option<&Value>,
    ) -> Result<(), String> {
        let base = self.allod().join("changesets");
        write_yaml(&base.join(format!("{}.yaml", short(hash))), cs)?;
        if let Some(evidence) = evidence {
            write_yaml(&base.join(format!("{}.evidence.yaml", short(hash))), evidence)?;
        }
        fs::write(self.allod().join("HEAD"), hash).map_err(|e| e.to_string())
    }

    pub fn read_changeset(&self, hash: &str) -> Result<Value, String> {
        read_yaml(
            &self
                .allod()
                .join("changesets")
                .join(format!("{}.yaml", short(hash))),
        )
    }

    pub fn read_evidence(&self, hash: &str) -> Result<Option<Value>, String> {
        let path = self
            .allod()
            .join("changesets")
            .join(format!("{}.evidence.yaml", short(hash)));
        if !path.exists() {
            return Ok(None);
        }
        read_yaml(&path).map(Some)
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
        write_yaml(
            &self
                .allod()
                .join("proposals")
                .join(format!("{}.yaml", short(hash))),
            cs,
        )
    }

    pub fn write_proposal_evidence(&self, hash: &str, evidence: &Value) -> Result<(), String> {
        write_yaml(
            &self
                .allod()
                .join("proposals")
                .join(format!("{}.evidence.yaml", short(hash))),
            evidence,
        )
    }

    pub fn read_proposal(&self, hash: &str) -> Result<Value, String> {
        read_yaml(
            &self
                .allod()
                .join("proposals")
                .join(format!("{}.yaml", short(hash))),
        )
    }

    pub fn read_proposal_evidence(&self, hash: &str) -> Result<Value, String> {
        let path = self
            .allod()
            .join("proposals")
            .join(format!("{}.evidence.yaml", short(hash)));
        if !path.exists() {
            let mut map = Mapping::new();
            map.insert(Value::String("decisions".into()), Value::Sequence(vec![]));
            map.insert(Value::String("envelopes".into()), Value::Sequence(vec![]));
            return Ok(Value::Mapping(map));
        }
        read_yaml(&path)
    }

    pub fn list_proposals(&self) -> Result<Vec<String>, String> {
        let dir = self.allod().join("proposals");
        let mut hashes = Vec::new();
        for entry in fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(stem) = name.strip_suffix(".yaml") {
                if !stem.ends_with(".evidence") {
                    hashes.push(format!("sha256:{stem}"));
                }
            }
        }
        hashes.sort();
        Ok(hashes)
    }

    pub fn remove_proposal(&self, hash: &str) -> Result<(), String> {
        let base = self.allod().join("proposals");
        let _ = fs::remove_file(base.join(format!("{}.yaml", short(hash))));
        let _ = fs::remove_file(base.join(format!("{}.evidence.yaml", short(hash))));
        Ok(())
    }
}
