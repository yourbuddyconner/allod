//! Schema compiler and projector.
//!
//! - `compile_schema_ops`: turns projection-form docs + policy into `create`
//!   operations on meta nodes (`meta/EntityType`, `meta/EdgeType`, `meta/Struct`,
//!   `meta/ValidationRule`, `meta/TaxonomyTerm`, `meta/Policy`).
//! - `project_schema`: inverse — walks the meta sub-graph of a folded State and
//!   reassembles projection-form `(name, Value)` documents, suitable for feeding
//!   back into `load_docs` or installing into a graph.
//!
//! ## Document naming convention (project_schema output)
//!
//! | Source                     | Output doc name        |
//! |----------------------------|------------------------|
//! | ontology package "foo"     | `"foo"`                |
//! | taxonomy "bar"             | `"bar-taxonomy"`       |
//! | policy node (any)          | `"policy"`             |
//!
//! These names exactly mirror the names that `flows::profile_from_dir` and
//! `profiles::embedded_profile` use when assembling a `ProfileSource`.

use crate::fold::State;
use crate::{bare, get_str};
use serde_yaml::Value;
use std::collections::{BTreeMap, BTreeSet};

// ─── helpers ────────────────────────────────────────────────────────────────

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

/// Build a `{ create: { kind: "node", id, type, attributes: {...} } }` op value.
fn create_node_op(id: &str, type_ref: &str, attributes: serde_yaml::Mapping) -> Value {
    mk(&[("create", mk(&[
        ("kind", s("node")),
        ("id",   s(id)),
        ("type", s(type_ref)),
        ("attributes", Value::Mapping(attributes)),
    ]))])
}

/// Serialise `val` to a canonical YAML string (definition attribute).
fn to_definition(val: &Value) -> Result<String, String> {
    serde_yaml::to_string(val).map_err(|e| e.to_string())
}

// ─── compile ────────────────────────────────────────────────────────────────

/// Compile projection-form documents (ontology/taxonomy YAML mappings plus optional
/// policy document) into `create` operations on meta nodes.
///
/// IDs are minted by `mint_id`; callers can supply `uuid4()` for live use or
/// deterministic IDs in test vectors.
///
/// The `imports` field of each ontology document is encoded as a
/// `list<string>` on each of that package's meta nodes, so that
/// `Registry::from_state` can reconstruct the package's import list.
///
/// If `policy` is `Some`, a `meta/Policy@1` node is created. If `None`, no policy
/// node is emitted — useful when compiling docs-only packages without overriding
/// the existing policy.
pub fn compile_schema_ops(
    docs: &[(String, Value)],
    policy: Option<&Value>,
    mint_id: &mut dyn FnMut() -> String,
) -> Result<Vec<Value>, String> {
    let mut ops: Vec<Value> = Vec::new();

    for (doc_name, doc) in docs {
        // ── ontology ────────────────────────────────────────────────────────
        if let Some(pkg_name) = doc.get("ontology").and_then(Value::as_str) {
            // Collect imports: each import entry has `{ ontology: "name", ... }`
            let imports: Vec<Value> = doc
                .get("imports")
                .and_then(Value::as_sequence)
                .map(|seq| {
                    seq.iter()
                        .filter_map(|entry| get_str(entry, "ontology"))
                        .map(|n| s(n))
                        .collect()
                })
                .unwrap_or_default();
            let imports_val = Value::Sequence(imports);

            // Entity types
            if let Some(ets) = doc.get("entity_types").and_then(Value::as_mapping) {
                for (tname, tdef) in ets {
                    let Some(tname) = tname.as_str() else { continue };
                    let definition = to_definition(tdef)?;
                    let mut attrs = serde_yaml::Mapping::new();
                    attrs.insert(s("name"), s(tname));
                    attrs.insert(s("package"), s(pkg_name));
                    attrs.insert(s("definition"), s(&definition));
                    if !imports_val.as_sequence().map_or(true, |seq| seq.is_empty()) {
                        attrs.insert(s("imports"), imports_val.clone());
                    }
                    ops.push(create_node_op(&mint_id(), "meta/EntityType@1", attrs));
                }
            }

            // Edge types
            if let Some(edges) = doc.get("edge_types").and_then(Value::as_mapping) {
                for (ename, edef) in edges {
                    let Some(ename) = ename.as_str() else { continue };
                    let definition = to_definition(edef)?;
                    let mut attrs = serde_yaml::Mapping::new();
                    attrs.insert(s("name"), s(ename));
                    attrs.insert(s("package"), s(pkg_name));
                    attrs.insert(s("definition"), s(&definition));
                    if !imports_val.as_sequence().map_or(true, |seq| seq.is_empty()) {
                        attrs.insert(s("imports"), imports_val.clone());
                    }
                    ops.push(create_node_op(&mint_id(), "meta/EdgeType@1", attrs));
                }
            }

            // Structs
            if let Some(structs) = doc.get("structs").and_then(Value::as_mapping) {
                for (sname, sdef) in structs {
                    let Some(sname) = sname.as_str() else { continue };
                    let definition = to_definition(sdef)?;
                    let mut attrs = serde_yaml::Mapping::new();
                    attrs.insert(s("name"), s(sname));
                    attrs.insert(s("package"), s(pkg_name));
                    attrs.insert(s("definition"), s(&definition));
                    if !imports_val.as_sequence().map_or(true, |seq| seq.is_empty()) {
                        attrs.insert(s("imports"), imports_val.clone());
                    }
                    ops.push(create_node_op(&mint_id(), "meta/Struct@1", attrs));
                }
            }

            // Validation rules
            if let Some(rules) = doc.get("validation_rules").and_then(Value::as_sequence) {
                for rule in rules {
                    let definition = to_definition(rule)?;
                    let rule_name = get_str(rule, "name").unwrap_or("unnamed");
                    let mut attrs = serde_yaml::Mapping::new();
                    attrs.insert(s("name"), s(rule_name));
                    attrs.insert(s("package"), s(pkg_name));
                    attrs.insert(s("definition"), s(&definition));
                    if !imports_val.as_sequence().map_or(true, |seq| seq.is_empty()) {
                        attrs.insert(s("imports"), imports_val.clone());
                    }
                    ops.push(create_node_op(&mint_id(), "meta/ValidationRule@1", attrs));
                }
            }

            let _ = doc_name; // used for naming context; names are reconstructed from content
        }
        // ── taxonomy ────────────────────────────────────────────────────────
        else if let Some(tax_name) = doc.get("taxonomy").and_then(Value::as_str) {
            if let Some(terms) = doc.get("terms").and_then(Value::as_sequence) {
                for term in terms {
                    let Some(tname) = get_str(term, "name") else { continue };
                    let parents: Vec<Value> = term
                        .get("parents")
                        .and_then(Value::as_sequence)
                        .cloned()
                        .unwrap_or_default();
                    let status = get_str(term, "status");
                    let mut attrs = serde_yaml::Mapping::new();
                    attrs.insert(s("name"), s(tname));
                    attrs.insert(s("taxonomy"), s(tax_name));
                    attrs.insert(s("parents"), Value::Sequence(parents));
                    if let Some(st) = status {
                        attrs.insert(s("status"), s(st));
                    }
                    ops.push(create_node_op(&mint_id(), "meta/TaxonomyTerm@1", attrs));
                }
            }
        }
    }

    // ── policy ──────────────────────────────────────────────────────────────
    if let Some(policy) = policy {
        let policy_name = get_str(policy, "policy").unwrap_or("policy");
        let definition = to_definition(policy)?;
        let mut attrs = serde_yaml::Mapping::new();
        attrs.insert(s("name"), s(policy_name));
        attrs.insert(s("definition"), s(&definition));
        ops.push(create_node_op(&mint_id(), "meta/Policy@1", attrs));
    }

    Ok(ops)
}

// ─── project ────────────────────────────────────────────────────────────────

/// Project the meta sub-graph of a folded `State` back to projection-form
/// documents.
///
/// Output is deterministic and sorted:
/// - Ontology docs named by package name (e.g. `"core"`, `"memory"`), sorted
///   alphabetically by package name.
/// - Taxonomy docs named `"<taxonomy>-taxonomy"`, sorted alphabetically.
/// - Policy docs named `"policy"` (only the first policy node found, sorted by
///   node id for stability; policies beyond the first are ignored here).
///
/// The output is suitable for round-tripping: `load_docs(project_schema(state))`
/// produces a registry equivalent to `Registry::from_state(state)`.
pub fn project_schema(state: &State) -> Result<Vec<(String, Value)>, String> {
    use crate::meta::is_meta_type;

    // Accumulators keyed by package or taxonomy name.
    let mut pkg_entity_types: BTreeMap<String, serde_yaml::Mapping> = BTreeMap::new();
    let mut pkg_edge_types: BTreeMap<String, serde_yaml::Mapping> = BTreeMap::new();
    let mut pkg_structs: BTreeMap<String, serde_yaml::Mapping> = BTreeMap::new();
    let mut pkg_rules: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut pkg_imports: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    // taxonomy_name -> list of term maps (as they appear in the `terms:` sequence)
    let mut tax_terms: BTreeMap<String, Vec<(String, Value)>> = BTreeMap::new();

    // policy: collect (node_id, policy_name, definition_str)
    let mut policies: Vec<(String, String, String)> = Vec::new();

    for ((kind, id), obj) in &state.objects {
        if kind != "node" || obj.deleted {
            continue;
        }
        let type_ref = match get_str(&obj.content, "type") {
            Some(t) => t,
            None => continue,
        };
        if !is_meta_type(type_ref) {
            continue;
        }
        let bare_type = bare(type_ref);

        let attrs = obj.content.get("attributes");
        let get_attr = |key: &str| -> Option<&str> {
            attrs.and_then(|a| get_str(a, key))
        };

        match bare_type {
            "meta/EntityType" => {
                let name = get_attr("name")
                    .ok_or_else(|| format!("meta/EntityType node {id} missing name"))?;
                let package = get_attr("package")
                    .ok_or_else(|| format!("meta/EntityType node {id} missing package"))?;
                let definition = get_attr("definition")
                    .ok_or_else(|| format!("meta/EntityType node {id} missing definition"))?;
                let def_val: Value = serde_yaml::from_str(definition)
                    .map_err(|e| format!("meta/EntityType node {id} bad definition: {e}"))?;
                pkg_entity_types
                    .entry(package.to_string())
                    .or_default()
                    .insert(Value::String(name.to_string()), def_val);
                collect_imports(attrs, package, &mut pkg_imports);
            }
            "meta/EdgeType" => {
                let name = get_attr("name")
                    .ok_or_else(|| format!("meta/EdgeType node {id} missing name"))?;
                let package = get_attr("package")
                    .ok_or_else(|| format!("meta/EdgeType node {id} missing package"))?;
                let definition = get_attr("definition")
                    .ok_or_else(|| format!("meta/EdgeType node {id} missing definition"))?;
                let def_val: Value = serde_yaml::from_str(definition)
                    .map_err(|e| format!("meta/EdgeType node {id} bad definition: {e}"))?;
                pkg_edge_types
                    .entry(package.to_string())
                    .or_default()
                    .insert(Value::String(name.to_string()), def_val);
                collect_imports(attrs, package, &mut pkg_imports);
            }
            "meta/Struct" => {
                let name = get_attr("name")
                    .ok_or_else(|| format!("meta/Struct node {id} missing name"))?;
                let package = get_attr("package")
                    .ok_or_else(|| format!("meta/Struct node {id} missing package"))?;
                let definition = get_attr("definition")
                    .ok_or_else(|| format!("meta/Struct node {id} missing definition"))?;
                let def_val: Value = serde_yaml::from_str(definition)
                    .map_err(|e| format!("meta/Struct node {id} bad definition: {e}"))?;
                pkg_structs
                    .entry(package.to_string())
                    .or_default()
                    .insert(Value::String(name.to_string()), def_val);
                collect_imports(attrs, package, &mut pkg_imports);
            }
            "meta/ValidationRule" => {
                let package = get_attr("package")
                    .ok_or_else(|| format!("meta/ValidationRule node {id} missing package"))?;
                let definition = get_attr("definition")
                    .ok_or_else(|| format!("meta/ValidationRule node {id} missing definition"))?;
                let def_val: Value = serde_yaml::from_str(definition)
                    .map_err(|e| format!("meta/ValidationRule node {id} bad definition: {e}"))?;
                pkg_rules
                    .entry(package.to_string())
                    .or_default()
                    .push(def_val);
                collect_imports(attrs, package, &mut pkg_imports);
            }
            "meta/TaxonomyTerm" => {
                let name = get_attr("name")
                    .ok_or_else(|| format!("meta/TaxonomyTerm node {id} missing name"))?;
                let taxonomy = get_attr("taxonomy")
                    .ok_or_else(|| format!("meta/TaxonomyTerm node {id} missing taxonomy"))?;
                let parents: Vec<Value> = attrs
                    .and_then(|a| a.get("parents"))
                    .and_then(Value::as_sequence)
                    .cloned()
                    .unwrap_or_default();
                let status = get_attr("status");

                let mut term_map = serde_yaml::Mapping::new();
                term_map.insert(s("name"), s(name));
                term_map.insert(s("parents"), Value::Sequence(parents));
                if let Some(st) = status {
                    term_map.insert(s("status"), s(st));
                }
                tax_terms
                    .entry(taxonomy.to_string())
                    .or_default()
                    .push((name.to_string(), Value::Mapping(term_map)));
            }
            "meta/Policy" => {
                let name = get_attr("name")
                    .ok_or_else(|| format!("meta/Policy node {id} missing name"))?;
                let definition = get_attr("definition")
                    .ok_or_else(|| format!("meta/Policy node {id} missing definition"))?;
                policies.push((id.clone(), name.to_string(), definition.to_string()));
            }
            _ => {}
        }
    }

    // Collect all package names (union of all package maps)
    let all_packages: BTreeSet<String> = pkg_entity_types
        .keys()
        .chain(pkg_edge_types.keys())
        .chain(pkg_structs.keys())
        .chain(pkg_rules.keys())
        .cloned()
        .collect();

    let mut result: Vec<(String, Value)> = Vec::new();

    // ── ontology docs ────────────────────────────────────────────────────────
    for pkg_name in &all_packages {
        let mut doc = serde_yaml::Mapping::new();
        doc.insert(s("ontology"), s(pkg_name));
        doc.insert(s("version"), Value::Number(1.into()));

        // imports: reconstruct as `[{ ontology: "name" }, ...]` sorted
        if let Some(imps) = pkg_imports.get(pkg_name) {
            if !imps.is_empty() {
                let import_seq: Vec<Value> = imps
                    .iter()
                    .map(|imp| mk(&[("ontology", s(imp))]))
                    .collect();
                doc.insert(s("imports"), Value::Sequence(import_seq));
            }
        }

        if let Some(ets) = pkg_entity_types.get(pkg_name) {
            doc.insert(s("entity_types"), Value::Mapping(ets.clone()));
        }
        if let Some(edges) = pkg_edge_types.get(pkg_name) {
            doc.insert(s("edge_types"), Value::Mapping(edges.clone()));
        }
        if let Some(structs) = pkg_structs.get(pkg_name) {
            doc.insert(s("structs"), Value::Mapping(structs.clone()));
        }
        if let Some(rules) = pkg_rules.get(pkg_name) {
            doc.insert(s("validation_rules"), Value::Sequence(rules.clone()));
        }

        result.push((pkg_name.clone(), Value::Mapping(doc)));
    }

    // ── taxonomy docs ────────────────────────────────────────────────────────
    for (tax_name, mut terms) in tax_terms {
        // Sort terms by name for determinism
        terms.sort_by(|(a, _), (b, _)| a.cmp(b));
        let term_seq: Vec<Value> = terms.into_iter().map(|(_, v)| v).collect();

        let mut doc = serde_yaml::Mapping::new();
        doc.insert(s("taxonomy"), s(&tax_name));
        doc.insert(s("version"), Value::Number(1.into()));
        doc.insert(s("terms"), Value::Sequence(term_seq));

        result.push((format!("{tax_name}-taxonomy"), Value::Mapping(doc)));
    }

    // ── policy doc ───────────────────────────────────────────────────────────
    // A graph carries exactly one policy node. Error if more than one exists.
    if policies.len() > 1 {
        let ids: Vec<&str> = policies.iter().map(|(id, _, _)| id.as_str()).collect();
        return Err(format!("multiple meta/Policy nodes: {} — a graph carries one policy", ids.join(", ")));
    }
    if let Some((_, _policy_name, definition)) = policies.into_iter().next() {
        let policy_val: Value = serde_yaml::from_str(&definition)
            .map_err(|e| format!("policy definition not valid YAML: {e}"))?;
        result.push(("policy".to_string(), policy_val));
    }

    // Sort the result: ontologies first (alphabetical), then taxonomies
    // (alphabetical), then policy. Since "policy" < "z..." and taxonomy names
    // end in "-taxonomy", we use a stable sort with a custom key.
    result.sort_by(|(a, _), (b, _)| {
        doc_sort_key(a).cmp(&doc_sort_key(b))
    });

    Ok(result)
}

/// Returns a sort key so that ontologies sort before taxonomies before policy.
fn doc_sort_key(name: &str) -> (u8, &str) {
    if name == "policy" {
        (2, name)
    } else if name.ends_with("-taxonomy") {
        (1, name)
    } else {
        (0, name)
    }
}

fn collect_imports(
    attrs: Option<&Value>,
    package: &str,
    pkg_imports: &mut BTreeMap<String, BTreeSet<String>>,
) {
    if let Some(imports_seq) = attrs
        .and_then(|a| a.get("imports"))
        .and_then(Value::as_sequence)
    {
        let set = pkg_imports.entry(package.to_string()).or_default();
        for imp_val in imports_seq {
            if let Some(imp_str) = imp_val.as_str() {
                set.insert(imp_str.to_string());
            }
        }
    }
}

// ─── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fold::State;
    use crate::registry::Registry;

    // ---- helpers ----

    fn seq_id() -> impl FnMut() -> String {
        let mut n = 0u32;
        move || {
            n += 1;
            format!("meta-id-{n:04}")
        }
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

    /// Apply a list of create-node ops to a State using meta_registry.
    fn apply_ops(ops: &[Value]) -> State {
        use crate::fold::Obj;
        use crate::model::revision_hash;

        let mut state = State::default();
        for op in ops {
            let Some(payload) = op.get("create") else { continue };
            let kind = get_str(payload, "kind").expect("kind").to_string();
            let id = get_str(payload, "id").expect("id").to_string();
            let rev = revision_hash(payload).expect("revision hash");
            state.objects.insert(
                (kind, id),
                Obj {
                    content: payload.clone(),
                    rev,
                    deleted: false,
                    redacted: false,
                },
            );
        }
        state
    }

    // ---- RED tests (Step 1) ----

    /// Compile memory profile docs → apply to State → Registry::from_state
    /// must have memory/Note with content + title attrs, plus workspace/scratch
    /// with workspace as a parent, plus the policy node present.
    #[test]
    fn compile_round_trip_memory_profile_registry() {
        let docs = memory_docs();
        let policy = memory_policy();

        let ops = compile_schema_ops(&docs, Some(&policy), &mut seq_id())
            .expect("compile_schema_ops must succeed");

        assert!(!ops.is_empty(), "must produce at least one op");

        let state = apply_ops(&ops);
        let reg = Registry::from_state(&state).expect("from_state must succeed");

        // memory/Note must resolve with content + title attrs
        let resolved = reg.resolve_type("memory/Note", None);
        assert!(resolved.is_some(), "memory/Note must resolve");
        let attrs = reg.collected_attrs("memory", "Note");
        assert!(attrs.contains_key("content"), "Note must have content attr");
        assert!(attrs.contains_key("title"), "Note must have title attr");
        let content_def = attrs.get("content").expect("content attr");
        assert_eq!(
            content_def.get("type").and_then(Value::as_str),
            Some("string"),
            "content must be type string"
        );
        let required = content_def.get("required").and_then(Value::as_bool) == Some(true);
        assert!(required, "content must be required");

        // workspace/scratch taxonomy term with workspace parent
        assert!(reg.term_exists("workspace/scratch"), "workspace/scratch must exist");
        let closure = reg.term_closure("workspace/scratch");
        assert!(closure.contains("workspace"), "closure must contain workspace");
        assert!(closure.contains("workspace/scratch"), "closure must contain itself");

        // core package must have resolved types
        let core_resolved = reg.resolve_type("core/Person", None);
        assert!(core_resolved.is_some(), "core/Person must resolve");

        // memory package must have imports pointing at core
        let mem_pkg = reg.packages.get("memory").expect("memory package");
        assert!(
            mem_pkg.imports.contains(&"core".to_string()),
            "memory package imports must include core, got: {:?}",
            mem_pkg.imports
        );
    }

    /// project_schema output must load via load_docs into an equivalent registry.
    #[test]
    fn project_schema_reloads_into_equivalent_registry() {
        let docs = memory_docs();
        let policy = memory_policy();

        let ops = compile_schema_ops(&docs, Some(&policy), &mut seq_id())
            .expect("compile_schema_ops must succeed");
        let state = apply_ops(&ops);

        let projected = project_schema(&state).expect("project_schema must succeed");
        assert!(!projected.is_empty(), "projection must produce docs");

        // Reload via load_docs
        let loaded = crate::loader::load_docs(&projected);
        assert!(
            loaded.issues.is_empty(),
            "load_docs must have no issues on projected docs: {:?}",
            loaded.issues.iter().map(|i| &i.message).collect::<Vec<_>>()
        );

        // Sampled registry assertions
        let reg = loaded.registry;
        let attrs = reg.collected_attrs("memory", "Note");
        assert!(attrs.contains_key("content"), "reloaded: Note.content");
        assert!(attrs.contains_key("title"), "reloaded: Note.title");

        assert!(reg.term_exists("workspace/scratch"), "reloaded: workspace/scratch");
        let closure = reg.term_closure("workspace/scratch");
        assert!(closure.contains("workspace"), "reloaded: scratch closure has workspace");

        assert!(reg.resolve_type("core/Person", None).is_some(), "reloaded: core/Person");
    }

    /// Two project_schema calls on the same state must produce identical output (byte stability).
    #[test]
    fn project_schema_is_byte_stable() {
        let docs = memory_docs();
        let policy = memory_policy();

        let ops = compile_schema_ops(&docs, Some(&policy), &mut seq_id())
            .expect("compile_schema_ops must succeed");
        let state = apply_ops(&ops);

        let p1 = project_schema(&state).expect("first projection");
        let p2 = project_schema(&state).expect("second projection");

        assert_eq!(p1.len(), p2.len(), "same number of docs");
        for ((n1, v1), (n2, v2)) in p1.iter().zip(p2.iter()) {
            assert_eq!(n1, n2, "same doc name");
            let s1 = serde_yaml::to_string(v1).expect("serialize p1");
            let s2 = serde_yaml::to_string(v2).expect("serialize p2");
            assert_eq!(s1, s2, "doc {n1} must be byte-stable");
        }
    }

    /// compile_schema_ops on a minimal ontology with one entity type produces
    /// exactly one EntityType op and one Policy op (for the policy).
    #[test]
    fn compile_minimal_ontology_produces_correct_ops() {
        let doc_yaml = r#"
ontology: test
version: 1
entity_types:
  Widget:
    attributes:
      label: { type: string, required: true }
"#;
        let policy_yaml = r#"policy: test-policy
version: 1
default_posture: restricted
roles: {}
rules: []
"#;
        let doc: Value = serde_yaml::from_str(doc_yaml).unwrap();
        let policy: Value = serde_yaml::from_str(policy_yaml).unwrap();

        let ops = compile_schema_ops(&[("test".to_string(), doc)], Some(&policy), &mut seq_id())
            .expect("compile must succeed");

        let types: Vec<&str> = ops
            .iter()
            .filter_map(|op| op.get("create"))
            .filter_map(|p| get_str(p, "type"))
            .collect();

        assert!(
            types.contains(&"meta/EntityType@1"),
            "must have EntityType op, got: {types:?}"
        );
        assert!(
            types.contains(&"meta/Policy@1"),
            "must have Policy op, got: {types:?}"
        );
        // Policy should be the only Policy
        assert_eq!(
            types.iter().filter(|&&t| t == "meta/Policy@1").count(),
            1,
            "exactly one Policy op"
        );
        // EntityType should be the only non-policy
        assert_eq!(
            types.iter().filter(|&&t| t == "meta/EntityType@1").count(),
            1,
            "exactly one EntityType op for Widget"
        );
    }

    /// Taxonomy terms compile to meta/TaxonomyTerm ops, one per term.
    #[test]
    fn compile_taxonomy_produces_term_ops() {
        let tax_yaml = r#"
taxonomy: workspace
version: 1
terms:
  - { name: workspace, parents: [] }
  - { name: workspace/scratch, parents: [workspace] }
"#;
        let policy_yaml = r#"policy: p
version: 1
default_posture: restricted
roles: {}
rules: []
"#;
        let tax: Value = serde_yaml::from_str(tax_yaml).unwrap();
        let policy: Value = serde_yaml::from_str(policy_yaml).unwrap();

        let ops = compile_schema_ops(&[("workspace-taxonomy".to_string(), tax)], Some(&policy), &mut seq_id())
            .expect("compile must succeed");

        let term_ops: Vec<_> = ops
            .iter()
            .filter_map(|op| op.get("create"))
            .filter(|p| get_str(p, "type") == Some("meta/TaxonomyTerm@1"))
            .collect();

        assert_eq!(term_ops.len(), 2, "two term ops for two terms");

        let term_names: Vec<&str> = term_ops
            .iter()
            .filter_map(|p| p.get("attributes"))
            .filter_map(|a| get_str(a, "name"))
            .collect();
        assert!(term_names.contains(&"workspace"), "workspace term");
        assert!(term_names.contains(&"workspace/scratch"), "workspace/scratch term");
    }

    /// project_schema emits ontology docs named by package, taxonomy docs named
    /// `<taxonomy>-taxonomy`, and policy doc named `policy`.
    #[test]
    fn project_schema_naming_convention() {
        let docs = memory_docs();
        let policy = memory_policy();

        let ops = compile_schema_ops(&docs, Some(&policy), &mut seq_id())
            .expect("compile_schema_ops must succeed");
        let state = apply_ops(&ops);

        let projected = project_schema(&state).expect("project_schema");
        let names: Vec<&str> = projected.iter().map(|(n, _)| n.as_str()).collect();

        // Must have ontology docs
        assert!(names.contains(&"core"), "must have 'core' doc");
        assert!(names.contains(&"memory"), "must have 'memory' doc");

        // Must have taxonomy doc named with -taxonomy suffix
        assert!(
            names.contains(&"memory-taxonomy"),
            "must have 'memory-taxonomy' doc, got: {names:?}"
        );

        // Must have policy doc
        assert!(names.contains(&"policy"), "must have 'policy' doc");
    }

    /// project_schema must error when the state contains multiple meta/Policy nodes.
    #[test]
    fn project_schema_errors_on_multiple_policy_nodes() {
        use crate::fold::Obj;
        use crate::model::revision_hash;

        let mut state = State::default();

        // Create two policy nodes
        for i in 0..2 {
            let payload = mk(&[
                ("kind", s("node")),
                ("id", s(&format!("policy-{i}"))),
                ("type", s("meta/Policy@1")),
                ("attributes", mk(&[
                    ("name", s(&format!("policy-{i}"))),
                    ("definition", s("{ default_posture: restricted, roles: {}, rules: [] }")),
                ])),
            ]);
            let rev = revision_hash(&payload).expect("revision hash");
            state.objects.insert(
                ("node".to_string(), format!("policy-{i}")),
                Obj {
                    content: payload,
                    rev,
                    deleted: false,
                    redacted: false,
                },
            );
        }

        let result = project_schema(&state);
        assert!(result.is_err(), "must error on multiple policy nodes");
        let err = result.unwrap_err();
        assert!(
            err.contains("multiple meta/Policy nodes"),
            "error message must mention multiple policy nodes, got: {err}"
        );
        assert!(err.contains("policy-0"), "error must name first policy node");
        assert!(err.contains("policy-1"), "error must name second policy node");
    }
}
