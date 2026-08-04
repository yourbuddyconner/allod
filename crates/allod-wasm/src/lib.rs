//! WASM bindings for allod-graph — the @allod/core npm package.
//!
//! One exported class (`AllodGraph`) wraps a `Graph` over a `MemStore`.
//! Every mutating method snapshots the store via `MemStore::dump()` and
//! invokes the JavaScript `persist` callback before resolving.

use allod_core::docstore::MemStore;
use allod_core::store::Graph;
use js_sys::{Array, Function, Promise};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

// ---- helpers ----------------------------------------------------------------

fn err(e: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&e.to_string())
}


/// Serialise a Rust value via serde-wasm-bindgen (no JSON round-trip).
fn to_js<T: serde::Serialize>(v: &T) -> Result<JsValue, JsValue> {
    serde_wasm_bindgen::to_value(v).map_err(|e| JsValue::from_str(&e.to_string()))
}

/// Recursively convert a `serde_yaml::Value` to a `serde_json::Value`.
///
/// Allod changesets use only JSON-compatible YAML types (strings, numbers,
/// booleans, sequences, and string-keyed mappings). Non-string keys are
/// stringified as a fallback.
fn yaml_value_to_json(v: &serde_yaml::Value) -> serde_json::Value {
    match v {
        serde_yaml::Value::Null => serde_json::Value::Null,
        serde_yaml::Value::Bool(b) => serde_json::Value::Bool(*b),
        serde_yaml::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                serde_json::Value::Number(serde_json::Number::from(i))
            } else if let Some(u) = n.as_u64() {
                serde_json::Value::Number(serde_json::Number::from(u))
            } else if let Some(f) = n.as_f64() {
                serde_json::Number::from_f64(f)
                    .map(serde_json::Value::Number)
                    .unwrap_or(serde_json::Value::Null)
            } else {
                serde_json::Value::Null
            }
        }
        serde_yaml::Value::String(s) => serde_json::Value::String(s.clone()),
        serde_yaml::Value::Sequence(seq) => {
            serde_json::Value::Array(seq.iter().map(yaml_value_to_json).collect())
        }
        serde_yaml::Value::Mapping(map) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in map {
                let key = match k {
                    serde_yaml::Value::String(s) => s.clone(),
                    other => format!("{other:?}"),
                };
                obj.insert(key, yaml_value_to_json(v));
            }
            serde_json::Value::Object(obj)
        }
        serde_yaml::Value::Tagged(tagged) => yaml_value_to_json(&tagged.value),
    }
}

/// Convert a `serde_yaml::Value` to a plain JS object.
///
/// Goes: serde_yaml::Value → serde_json::Value → JSON string → JS.parse().
/// Using js_sys::JSON::parse avoids serde-wasm-bindgen's enum-tagging behaviour
/// for serde_json::Value.
fn yaml_to_js(v: &serde_yaml::Value) -> Result<JsValue, JsValue> {
    let json_val = yaml_value_to_json(v);
    let json_str =
        serde_json::to_string(&json_val).map_err(|e| JsValue::from_str(&e.to_string()))?;
    js_sys::JSON::parse(&json_str)
}

/// Convert a JsValue (JS array or object) to `serde_yaml::Value`.
///
/// serde-wasm-bindgen deserialises into serde_json-style values so we go:
///   JsValue → serde_json::Value → JSON string → serde_yaml::Value
fn js_to_yaml(v: JsValue) -> Result<serde_yaml::Value, JsValue> {
    let json_val: serde_json::Value =
        serde_wasm_bindgen::from_value(v).map_err(|e| JsValue::from_str(&e.to_string()))?;
    let json_str =
        serde_json::to_string(&json_val).map_err(|e| JsValue::from_str(&e.to_string()))?;
    serde_yaml::from_str(&json_str).map_err(|e| JsValue::from_str(&e.to_string()))
}

/// Convert a JsValue that is a JS Array into `Vec<serde_yaml::Value>`.
fn js_array_to_yaml_vec(v: JsValue) -> Result<Vec<serde_yaml::Value>, JsValue> {
    match js_to_yaml(v)? {
        serde_yaml::Value::Sequence(seq) => Ok(seq),
        other => Err(JsValue::from_str(&format!(
            "expected array, got {:?}",
            other
        ))),
    }
}

/// Convert a JS `Array<[string, string]>` (the dump/load format) into
/// `Vec<(String, String)>`.
fn array_to_pairs(arr: &JsValue) -> Result<Vec<(String, String)>, JsValue> {
    let arr = Array::from(arr);
    let mut out = Vec::new();
    for i in 0..arr.length() {
        let pair = Array::from(&arr.get(i));
        let k = pair.get(0).as_string().ok_or_else(|| JsValue::from_str("pair[0] not a string"))?;
        let v = pair.get(1).as_string().ok_or_else(|| JsValue::from_str("pair[1] not a string"))?;
        out.push((k, v));
    }
    Ok(out)
}

/// Convert `Vec<(String, String)>` to a JS `Array<[string, string]>`.
fn pairs_to_array(pairs: &[(String, String)]) -> JsValue {
    let outer = Array::new();
    for (k, v) in pairs {
        let inner = Array::new();
        inner.push(&JsValue::from_str(k));
        inner.push(&JsValue::from_str(v));
        outer.push(&inner);
    }
    outer.into()
}

// ---- AllodGraph -------------------------------------------------------------

#[wasm_bindgen]
pub struct AllodGraph {
    graph: Graph,
    store: std::sync::Arc<MemStore>,
    persist: Function,
}

#[wasm_bindgen]
impl AllodGraph {
    /// `docs`: `Array<[path, text]>` hydrating the MemStore (empty for a new graph).
    /// `persist`: `async (dump: Array<[path, text]>) => void` — called and awaited
    /// after every mutating call before that call resolves.
    #[wasm_bindgen(constructor)]
    pub fn new(docs: JsValue, persist: Function) -> Result<AllodGraph, JsValue> {
        let store = std::sync::Arc::new(MemStore::new());
        if !docs.is_null() && !docs.is_undefined() {
            let pairs = array_to_pairs(&docs)?;
            store.load(pairs);
        }
        let shared = SharedMemStore(store.clone());
        let graph = Graph::with_store(Box::new(shared));
        Ok(AllodGraph { graph, store, persist })
    }

    async fn do_persist(&self) -> Result<(), JsValue> {
        let dump = self.store.dump();
        let arr = pairs_to_array(&dump);
        // Call persist(arr) — may return a Promise or a plain value.
        let result = self.persist.call1(&JsValue::NULL, &arr)?;
        if result.is_instance_of::<Promise>() {
            JsFuture::from(Promise::from(result)).await?;
        }
        Ok(())
    }

    // ---- Mutating methods ---------------------------------------------------

    pub async fn init(&mut self, owner: String, profile: String) -> Result<JsValue, JsValue> {
        let ps = allod_graph::profiles::embedded_profile(&profile).map_err(err)?;
        let res = allod_graph::flows::init(&self.graph, &owner, ps).map_err(err)?;
        self.do_persist().await?;
        to_js(&InitResultJs { graph_id: res.graph_id, owner: res.owner })
    }

    pub async fn principal_add(
        &mut self,
        name: String,
        kind: String,
        by: String,
    ) -> Result<JsValue, JsValue> {
        let res = allod_graph::flows::principal_add(&self.graph, &name, &kind, &by).map_err(err)?;
        self.do_persist().await?;
        to_js(&PrincipalAddedJs {
            node_id: res.node_id,
            admission: res.admission,
        })
    }

    pub async fn commit(
        &mut self,
        author: String,
        intent: String,
        ops: JsValue,
        envelopes: JsValue,
    ) -> Result<JsValue, JsValue> {
        let ops_vec = js_array_to_yaml_vec(ops)?;
        let envelopes_vec = js_array_to_yaml_vec(envelopes)?;
        let res = allod_graph::ops::commit(&self.graph, &author, &intent, ops_vec, envelopes_vec)
            .map_err(err)?;
        self.do_persist().await?;
        to_js(&res)
    }

    pub async fn note(&mut self, agent: String, content: String) -> Result<JsValue, JsValue> {
        let res = allod_graph::flows::note(&self.graph, &agent, &content).map_err(err)?;
        self.do_persist().await?;
        to_js(&NoteResultJs {
            note_id: res.note_id,
            admission: res.admission,
        })
    }

    pub async fn propose_preference(
        &mut self,
        agent: String,
        statement: String,
        strength: String,
        from_note: Option<String>,
    ) -> Result<JsValue, JsValue> {
        let res = allod_graph::flows::propose_preference(
            &self.graph,
            &agent,
            &statement,
            &strength,
            from_note.as_deref(),
        )
        .map_err(err)?;
        self.do_persist().await?;
        to_js(&ProposalResultJs {
            hash: res.hash,
            admission: res.admission,
        })
    }

    pub async fn decide(
        &mut self,
        hash: String,
        by: String,
        verdict: String,
    ) -> Result<JsValue, JsValue> {
        let res = allod_graph::flows::decide(&self.graph, &hash, &by, &verdict).map_err(err)?;
        self.do_persist().await?;
        to_js(&res)
    }

    pub async fn classify(
        &mut self,
        node_id: String,
        term: String,
        by: String,
        basis: String,
    ) -> Result<JsValue, JsValue> {
        let res = allod_graph::flows::classify(&self.graph, &node_id, &term, &by, &basis)
            .map_err(err)?;
        self.do_persist().await?;
        to_js(&res)
    }

    /// Install a schema package into the graph as meta-typed nodes.
    ///
    /// `docs_yaml` is a YAML mapping of `{name: doc}` pairs (one entry per
    /// ontology / taxonomy document). `by` is the principal name performing
    /// the installation. Returns the admission outcome as a JsValue.
    pub async fn install_package(
        &mut self,
        docs_yaml: String,
        by: String,
    ) -> Result<JsValue, JsValue> {
        let raw: serde_yaml::Value =
            serde_yaml::from_str(&docs_yaml).map_err(|e| err(e))?;
        let map = raw
            .as_mapping()
            .ok_or_else(|| err("docs_yaml must be a YAML mapping {name: doc}"))?;
        let docs: Vec<(String, serde_yaml::Value)> = map
            .iter()
            .map(|(k, v)| {
                let name = k.as_str().unwrap_or("unknown").to_string();
                (name, v.clone())
            })
            .collect();
        let res =
            allod_graph::flows::install_package(&self.graph, &docs, None, &by)
                .map_err(err)?;
        self.do_persist().await?;
        to_js(&res)
    }

    // ---- Read-only methods --------------------------------------------------

    pub fn proposals(&self) -> Result<JsValue, JsValue> {
        let res = allod_graph::flows::proposals(&self.graph).map_err(err)?;
        to_js(&res)
    }

    pub fn log(&self) -> Result<JsValue, JsValue> {
        let res = allod_graph::flows::log(&self.graph).map_err(err)?;
        to_js(&res)
    }

    pub fn state(&self) -> Result<JsValue, JsValue> {
        let res = allod_graph::flows::state(&self.graph).map_err(err)?;
        to_js(&res)
    }

    pub fn verify(&self) -> Result<JsValue, JsValue> {
        let res = allod_graph::flows::verify(&self.graph).map_err(err)?;
        to_js(&res)
    }

    pub fn describe_schema(&self) -> Result<JsValue, JsValue> {
        let res = allod_graph::schema::describe(&self.graph).map_err(err)?;
        to_js(&res)
    }

    pub fn export_md(&self) -> Result<JsValue, JsValue> {
        let res = allod_graph::md::export_docs(&self.graph).map_err(err)?;
        Ok(pairs_to_array(&res))
    }

    /// Return the current revision hash (`rev`) for a live node, or `null`
    /// if the node is not found. The rev is needed as the `prior` field
    /// in update operations.
    pub fn node_rev(&self, node_id: String) -> Result<JsValue, JsValue> {
        let state = self.graph.fold().map_err(err)?;
        let key = ("node".to_string(), node_id);
        match state.objects.get(&key) {
            Some(obj) if !obj.deleted => Ok(JsValue::from_str(&obj.rev)),
            _ => Ok(JsValue::NULL),
        }
    }

    /// Return the full proposal changeset for `hash` (the value stored in
    /// proposals/<short>.yaml), serialised as a plain JS object, or an error
    /// if the proposal does not exist.
    pub fn proposal_get(&self, hash: String) -> Result<JsValue, JsValue> {
        let cs = self.graph.read_proposal(&hash).map_err(err)?;
        yaml_to_js(&cs)
    }

    /// Return the live object `{ content, rev, deleted }` from fold state for
    /// (`kind`, `id`), or `null` if absent. `kind` is "node", "edge", or
    /// "classification".
    pub fn object_get(&self, kind: String, id: String) -> Result<JsValue, JsValue> {
        let state = self.graph.fold().map_err(err)?;
        let key = (kind, id);
        match state.objects.get(&key) {
            None => Ok(JsValue::NULL),
            Some(obj) => {
                // Build a serde_yaml::Value mapping {content, rev, deleted} then
                // convert to plain JS via yaml_to_js (YAML→text→serde_json→JsValue).
                let mut map = serde_yaml::Mapping::new();
                map.insert(
                    serde_yaml::Value::String("content".into()),
                    obj.content.clone(),
                );
                map.insert(
                    serde_yaml::Value::String("rev".into()),
                    serde_yaml::Value::String(obj.rev.clone()),
                );
                map.insert(
                    serde_yaml::Value::String("deleted".into()),
                    serde_yaml::Value::Bool(obj.deleted),
                );
                yaml_to_js(&serde_yaml::Value::Mapping(map))
            }
        }
    }

    /// Return `{ classifications, edges_out, edges_in }` for the node with the
    /// given bare UUID. Walks the fold state to collect:
    ///   - `classifications`: all live classification objects whose `subject == "node:<id>"`
    ///   - `edges_out`: all live edge objects whose `from == "node:<id>"`
    ///   - `edges_in`:  all live edge objects whose `to   == "node:<id>"`
    ///
    /// Returns `null` if the node is not live in fold state.
    pub fn entity_context(&self, node_id: String) -> Result<JsValue, JsValue> {
        let state = self.graph.fold().map_err(err)?;
        let node_ref = format!("node:{node_id}");

        // Verify the node exists and is live
        if state.get_live("node", &node_id).is_none() {
            return Ok(JsValue::NULL);
        }

        let mut classifications: Vec<serde_yaml::Value> = Vec::new();
        let mut edges_out: Vec<serde_yaml::Value> = Vec::new();
        let mut edges_in: Vec<serde_yaml::Value> = Vec::new();

        for ((kind, id), obj) in &state.objects {
            if obj.deleted {
                continue;
            }
            match kind.as_str() {
                "classification" => {
                    let subject = obj.content.get("subject")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if subject == node_ref {
                        let mut entry = serde_yaml::Mapping::new();
                        for field in ["term", "asserted_by", "basis"] {
                            if let Some(v) = obj.content.get(field) {
                                entry.insert(
                                    serde_yaml::Value::String(field.to_string()),
                                    v.clone(),
                                );
                            }
                        }
                        classifications.push(serde_yaml::Value::Mapping(entry));
                    }
                }
                "edge" => {
                    let from = obj.content.get("from")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let to = obj.content.get("to")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let etype = obj.content.get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let attrs = obj.content.get("attributes")
                        .cloned()
                        .unwrap_or(serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));

                    if from == node_ref {
                        let mut entry = serde_yaml::Mapping::new();
                        entry.insert(serde_yaml::Value::String("id".into()), serde_yaml::Value::String(id.clone()));
                        entry.insert(serde_yaml::Value::String("type".into()), serde_yaml::Value::String(etype.to_string()));
                        entry.insert(serde_yaml::Value::String("to".into()), serde_yaml::Value::String(to.to_string()));
                        entry.insert(serde_yaml::Value::String("attributes".into()), attrs.clone());
                        edges_out.push(serde_yaml::Value::Mapping(entry));
                    }
                    if to == node_ref {
                        let mut entry = serde_yaml::Mapping::new();
                        entry.insert(serde_yaml::Value::String("id".into()), serde_yaml::Value::String(id.clone()));
                        entry.insert(serde_yaml::Value::String("type".into()), serde_yaml::Value::String(etype.to_string()));
                        entry.insert(serde_yaml::Value::String("from".into()), serde_yaml::Value::String(from.to_string()));
                        entry.insert(serde_yaml::Value::String("attributes".into()), attrs);
                        edges_in.push(serde_yaml::Value::Mapping(entry));
                    }
                }
                _ => {}
            }
        }

        let mut result = serde_yaml::Mapping::new();
        result.insert(
            serde_yaml::Value::String("classifications".into()),
            serde_yaml::Value::Sequence(classifications),
        );
        result.insert(
            serde_yaml::Value::String("edges_out".into()),
            serde_yaml::Value::Sequence(edges_out),
        );
        result.insert(
            serde_yaml::Value::String("edges_in".into()),
            serde_yaml::Value::Sequence(edges_in),
        );
        yaml_to_js(&serde_yaml::Value::Mapping(result))
    }

    /// Re-evaluate the admission policy for the proposal identified by `hash`
    /// and return the matched rule names from the checklist. Returns an empty
    /// array if the proposal does not exist or policy evaluation fails.
    pub fn proposal_checklist(&self, hash: String) -> Result<JsValue, JsValue> {
        use allod_core::{get_str, policy};

        let cs = match self.graph.read_proposal(&hash) {
            Ok(cs) => cs,
            Err(_) => return to_js(&Vec::<String>::new()),
        };
        let reg = match self.graph.registry() {
            Ok(r) => r,
            Err(_) => return to_js(&Vec::<String>::new()),
        };
        let policy_doc = match self.graph.policy() {
            Ok(p) => p,
            Err(_) => return to_js(&Vec::<String>::new()),
        };
        let state = match self.graph.fold() {
            Ok(s) => s,
            Err(_) => return to_js(&Vec::<String>::new()),
        };
        let author_ref = get_str(
            cs.get("author").unwrap_or(&serde_yaml::Value::Null),
            "principal",
        )
        .unwrap_or("?")
        .to_string();
        let author_kind = state
            .find_principal(&author_ref)
            .map(|(kind, _)| kind.to_string())
            .unwrap_or_else(|| "agent".to_string());
        let checklist = match policy::evaluate(&reg, &policy_doc, &state, &cs, &author_kind) {
            Ok(c) => c,
            Err(_) => return to_js(&Vec::<String>::new()),
        };
        let rules: Vec<String> = checklist.matched_rules.into_iter().collect();
        to_js(&rules)
    }
}

// ---- Shared MemStore bridge -------------------------------------------------

/// Forwards all DocStore calls to an Arc<MemStore>, allowing the store
/// to be referenced from both AllodGraph.store and the Graph itself.
struct SharedMemStore(std::sync::Arc<MemStore>);

impl allod_core::docstore::DocStore for SharedMemStore {
    fn read(&self, path: &str) -> Result<Option<String>, String> {
        self.0.read(path)
    }
    fn write(&self, path: &str, text: &str) -> Result<(), String> {
        self.0.write(path, text)
    }
    fn list(&self, dir: &str) -> Result<Vec<String>, String> {
        self.0.list(dir)
    }
    fn remove(&self, path: &str) -> Result<(), String> {
        self.0.remove(path)
    }
}

// ---- Serialisable wrappers --------------------------------------------------
// allod-graph result types that lack #[derive(Serialize)] get thin wrappers.

#[derive(serde::Serialize)]
struct InitResultJs {
    graph_id: String,
    owner: String,
}

#[derive(serde::Serialize)]
struct PrincipalAddedJs {
    node_id: String,
    admission: allod_graph::ops::Admission,
}

#[derive(serde::Serialize)]
struct NoteResultJs {
    note_id: String,
    admission: allod_graph::ops::Admission,
}

#[derive(serde::Serialize)]
struct ProposalResultJs {
    hash: String,
    admission: allod_graph::ops::Admission,
}

