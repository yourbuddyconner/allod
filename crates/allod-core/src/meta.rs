//! The meta-ontology: the fixed point that describes itself.
//!
//! Meta types are the schema for schema. They are compiled into the
//! registry and never written to a log. `from_state` reads them back
//! out of a folded state so the registry can be reconstructed from
//! persisted graph state.

use crate::fold::State;
use crate::registry::Registry;
use crate::{bare, get_str};
use serde_yaml::Value;

/// The canonical meta-package name.
pub const META_PACKAGE: &str = "meta";

/// All meta type names (unversioned, package-qualified).
pub const META_TYPES: &[&str] = &[
    "meta/EntityType",
    "meta/EdgeType",
    "meta/Struct",
    "meta/TaxonomyTerm",
    "meta/ValidationRule",
    "meta/Policy",
];

/// True when the type ref (version suffix allowed) is a meta type.
pub fn is_meta_type(type_ref: &str) -> bool {
    let b = bare(type_ref);
    META_TYPES.contains(&b)
}

/// A Registry pre-seeded with the meta package only.
///
/// The meta package is the fixed point: it declares the types used to
/// store all other schema. It is never serialised into a log; this
/// function is the authoritative definition.
pub fn meta_registry() -> Registry {
    let doc: Value = serde_yaml::from_str(META_ONTOLOGY_YAML).expect("meta ontology is valid");
    let mut reg = Registry::default();
    assert!(
        reg.register_ontology(&doc),
        "meta ontology must register cleanly"
    );
    reg
}

/// The canonical YAML definition of the meta package (projection form).
///
/// Attribute schema for each meta type:
/// - `name: string` (required) — bare name within its package, e.g. "Person"
/// - `package: string` (required for EntityType/EdgeType/Struct/ValidationRule) —
///   the owning package, e.g. "core"
/// - `version: int` (optional)
/// - `definition: string` (required) — YAML mapping for the element
/// - TaxonomyTerm additionally: `taxonomy: string` (required), `parents: list<string>`,
///   `status: enum<active|deprecated>`
/// - Policy: only `name` and `definition`
const META_ONTOLOGY_YAML: &str = r#"
ontology: meta
version: 1

entity_types:

  EntityType:
    attributes:
      name:       { type: string, required: true }
      package:    { type: string, required: true }
      version:    { type: int }
      definition: { type: string, required: true }

  EdgeType:
    attributes:
      name:       { type: string, required: true }
      package:    { type: string, required: true }
      version:    { type: int }
      definition: { type: string, required: true }

  Struct:
    attributes:
      name:       { type: string, required: true }
      package:    { type: string, required: true }
      version:    { type: int }
      definition: { type: string, required: true }

  TaxonomyTerm:
    attributes:
      name:     { type: string, required: true }
      taxonomy: { type: string, required: true }
      version:  { type: int }
      parents:  { type: list<string> }
      status:   { type: "enum<active|deprecated>" }
      definition: { type: string, required: true }

  ValidationRule:
    attributes:
      name:       { type: string, required: true }
      package:    { type: string, required: true }
      version:    { type: int }
      definition: { type: string, required: true }

  Policy:
    attributes:
      name:       { type: string, required: true }
      definition: { type: string, required: true }
"#;

impl Registry {
    /// Derive a full Registry from folded state.
    ///
    /// Seeds with `meta_registry()`, then walks every live, non-deleted
    /// node whose `content.type` is a meta type (except `meta/Policy`,
    /// which is loaded separately by the policy engine). Each node's
    /// `definition` attribute is a canonical YAML string for that element;
    /// identity comes from `name` + `package` (or `taxonomy` for terms).
    ///
    /// Returns `Err` naming the node id on any unparseable definition.
    pub fn from_state(state: &State) -> Result<Registry, String> {
        let mut reg = meta_registry();

        // Collect packages and taxonomies to build incrementally.
        // We accumulate meta nodes by package/taxonomy, then assemble them.
        use std::collections::HashMap;

        // package_name -> (entity_types, edge_types, structs, rules, imports)
        // represented as serde_yaml mappings being built up
        let mut pkg_entity_types: HashMap<String, serde_yaml::Mapping> = HashMap::new();
        let mut pkg_edge_types: HashMap<String, serde_yaml::Mapping> = HashMap::new();
        let mut pkg_structs: HashMap<String, serde_yaml::Mapping> = HashMap::new();
        let mut pkg_rules: HashMap<String, Vec<Value>> = HashMap::new();

        // taxonomy_name -> terms vec
        let mut tax_terms: HashMap<String, Vec<Value>> = HashMap::new();

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
            // Skip policy nodes — handled separately
            if bare_type == "meta/Policy" {
                continue;
            }

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
                }
                "meta/ValidationRule" => {
                    let package = get_attr("package")
                        .ok_or_else(|| format!("meta/ValidationRule node {id} missing package"))?;
                    let definition = get_attr("definition")
                        .ok_or_else(|| {
                            format!("meta/ValidationRule node {id} missing definition")
                        })?;
                    let def_val: Value = serde_yaml::from_str(definition).map_err(|e| {
                        format!("meta/ValidationRule node {id} bad definition: {e}")
                    })?;
                    pkg_rules
                        .entry(package.to_string())
                        .or_default()
                        .push(def_val);
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
                    term_map.insert(
                        Value::String("name".into()),
                        Value::String(name.to_string()),
                    );
                    term_map.insert(
                        Value::String("parents".into()),
                        Value::Sequence(parents),
                    );
                    if let Some(s) = status {
                        term_map.insert(
                            Value::String("status".into()),
                            Value::String(s.to_string()),
                        );
                    }
                    tax_terms
                        .entry(taxonomy.to_string())
                        .or_default()
                        .push(Value::Mapping(term_map));
                }
                _ => {}
            }
        }

        // Assemble packages
        let all_packages: std::collections::BTreeSet<String> = pkg_entity_types
            .keys()
            .chain(pkg_edge_types.keys())
            .chain(pkg_structs.keys())
            .chain(pkg_rules.keys())
            .cloned()
            .collect();

        for pkg_name in all_packages {
            let entity_types = pkg_entity_types
                .remove(&pkg_name)
                .map(Value::Mapping)
                .unwrap_or(Value::Null);
            let edge_types = pkg_edge_types
                .remove(&pkg_name)
                .map(Value::Mapping)
                .unwrap_or(Value::Null);
            let structs = pkg_structs
                .remove(&pkg_name)
                .map(Value::Mapping)
                .unwrap_or(Value::Null);
            let rules = pkg_rules
                .remove(&pkg_name)
                .map(Value::Sequence)
                .unwrap_or(Value::Null);

            insert_package(&mut reg, &pkg_name, entity_types, edge_types, structs, rules);
        }

        // Assemble taxonomies
        for (tax_name, terms) in tax_terms {
            insert_taxonomy(&mut reg, &tax_name, terms);
        }

        Ok(reg)
    }
}

/// Insert a fully-assembled package into the registry.
/// Shared by `register_ontology` and `from_state` so they cannot drift.
pub(crate) fn insert_package(
    reg: &mut Registry,
    name: &str,
    types: Value,
    edges: Value,
    structs: Value,
    rules: Value,
) {
    use crate::registry::Package;
    reg.packages.insert(
        name.to_string(),
        Package {
            types,
            edges,
            structs,
            rules,
            imports: Vec::new(),
        },
    );
}

/// Insert a taxonomy with already-assembled terms into the registry.
/// Shared by `register_taxonomy` and `from_state` so they cannot drift.
pub(crate) fn insert_taxonomy(reg: &mut Registry, name: &str, terms: Vec<Value>) {
    use crate::registry::Taxonomy;
    use std::collections::HashMap;
    let mut term_map: HashMap<String, Vec<String>> = HashMap::new();
    for term in &terms {
        if let Some(tname) = get_str(term, "name") {
            let parents = term
                .get("parents")
                .and_then(Value::as_sequence)
                .map(|p| {
                    p.iter()
                        .filter_map(Value::as_str)
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default();
            term_map.insert(tname.to_string(), parents);
        }
    }
    reg.taxonomies
        .insert(name.to_string(), Taxonomy { terms: term_map });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fold::{Obj, State};

    #[test]
    fn meta_registry_resolves_entity_type() {
        let reg = meta_registry();
        let resolved = reg.resolve_type("meta/EntityType", None);
        assert!(
            resolved.is_some(),
            "meta_registry must resolve meta/EntityType"
        );
        let (pkg, name, _) = resolved.unwrap();
        assert_eq!(pkg, "meta");
        assert_eq!(name, "EntityType");
    }

    #[test]
    fn meta_entity_type_has_name_and_definition_attrs() {
        let reg = meta_registry();
        let attrs = reg.collected_attrs("meta", "EntityType");
        assert!(
            attrs.contains_key("name"),
            "meta/EntityType must declare name attribute"
        );
        assert!(
            attrs.contains_key("definition"),
            "meta/EntityType must declare definition attribute"
        );
        let name_required = attrs
            .get("name")
            .and_then(|a| a.get("required"))
            .and_then(|v| v.as_bool())
            == Some(true);
        assert!(name_required, "name must be required");
        let def_required = attrs
            .get("definition")
            .and_then(|a| a.get("required"))
            .and_then(|v| v.as_bool())
            == Some(true);
        assert!(def_required, "definition must be required");
    }

    #[test]
    fn is_meta_type_recognises_meta_types() {
        assert!(is_meta_type("meta/EntityType@1"), "versioned meta type");
        assert!(is_meta_type("meta/TaxonomyTerm"), "unversioned");
        assert!(!is_meta_type("core/Person"), "non-meta type");
        assert!(!is_meta_type("meta/Unknown"), "unknown meta name");
    }

    fn make_node(id: &str, type_ref: &str, attrs: serde_yaml::Mapping) -> Obj {
        let mut content = serde_yaml::Mapping::new();
        content.insert(
            Value::String("kind".into()),
            Value::String("node".into()),
        );
        content.insert(Value::String("id".into()), Value::String(id.into()));
        content.insert(
            Value::String("type".into()),
            Value::String(type_ref.into()),
        );
        content.insert(
            Value::String("attributes".into()),
            Value::Mapping(attrs),
        );
        Obj {
            content: Value::Mapping(content),
            rev: "test-rev".into(),
            deleted: false,
            redacted: false,
        }
    }

    #[test]
    fn from_state_round_trips_entity_type() {
        // Build a State containing a meta/EntityType node for core/Person
        let mut state = State::default();

        let person_def = r#"
attributes:
  name:        { type: string, required: true }
  emails:      { type: list<string> }
"#;

        let mut attrs = serde_yaml::Mapping::new();
        attrs.insert(
            Value::String("name".into()),
            Value::String("Person".into()),
        );
        attrs.insert(
            Value::String("package".into()),
            Value::String("core".into()),
        );
        attrs.insert(
            Value::String("definition".into()),
            Value::String(person_def.into()),
        );

        state.objects.insert(
            ("node".into(), "meta-person-1".into()),
            make_node("meta-person-1", "meta/EntityType@1", attrs),
        );

        let reg = Registry::from_state(&state).expect("from_state must succeed");

        // core/Person must resolve
        let resolved = reg.resolve_type("core/Person", None);
        assert!(
            resolved.is_some(),
            "core/Person must resolve after from_state"
        );
        let attrs = reg.collected_attrs("core", "Person");
        assert!(attrs.contains_key("name"), "Person must have name attribute");
    }

    #[test]
    fn from_state_round_trips_taxonomy_term() {
        let mut state = State::default();

        // A root term "workspace"
        let mut root_attrs = serde_yaml::Mapping::new();
        root_attrs.insert(
            Value::String("name".into()),
            Value::String("workspace".into()),
        );
        root_attrs.insert(
            Value::String("taxonomy".into()),
            Value::String("workspace".into()),
        );
        root_attrs.insert(
            Value::String("parents".into()),
            Value::Sequence(vec![]),
        );
        root_attrs.insert(
            Value::String("definition".into()),
            Value::String("{}".into()),
        );

        state.objects.insert(
            ("node".into(), "term-workspace".into()),
            make_node(
                "term-workspace",
                "meta/TaxonomyTerm@1",
                root_attrs,
            ),
        );

        // A child term "workspace/scratch" with parent "workspace"
        let mut child_attrs = serde_yaml::Mapping::new();
        child_attrs.insert(
            Value::String("name".into()),
            Value::String("workspace/scratch".into()),
        );
        child_attrs.insert(
            Value::String("taxonomy".into()),
            Value::String("workspace".into()),
        );
        child_attrs.insert(
            Value::String("parents".into()),
            Value::Sequence(vec![Value::String("workspace".into())]),
        );
        child_attrs.insert(
            Value::String("definition".into()),
            Value::String("{}".into()),
        );

        state.objects.insert(
            ("node".into(), "term-workspace-scratch".into()),
            make_node(
                "term-workspace-scratch",
                "meta/TaxonomyTerm@1",
                child_attrs,
            ),
        );

        let reg = Registry::from_state(&state).expect("from_state must succeed");

        assert!(
            reg.term_exists("workspace/scratch"),
            "workspace/scratch must exist"
        );
        assert!(
            reg.term_exists("workspace"),
            "workspace must exist"
        );

        // Closure of workspace/scratch must include workspace
        let closure = reg.term_closure("workspace/scratch");
        assert!(
            closure.contains("workspace"),
            "closure must contain parent workspace"
        );
        assert!(
            closure.contains("workspace/scratch"),
            "closure must contain term itself"
        );
    }

    #[test]
    fn from_state_skips_policy_nodes() {
        let mut state = State::default();

        let mut attrs = serde_yaml::Mapping::new();
        attrs.insert(
            Value::String("name".into()),
            Value::String("my-policy".into()),
        );
        attrs.insert(
            Value::String("definition".into()),
            Value::String("{}".into()),
        );

        state.objects.insert(
            ("node".into(), "policy-1".into()),
            make_node("policy-1", "meta/Policy@1", attrs),
        );

        // Should succeed (policy skipped, not error)
        let reg = Registry::from_state(&state).expect("must succeed even with policy nodes");
        // No non-meta packages added
        assert!(!reg.packages.contains_key(""), "no empty package");
    }

    #[test]
    fn from_state_errors_on_bad_definition() {
        let mut state = State::default();

        let mut attrs = serde_yaml::Mapping::new();
        attrs.insert(
            Value::String("name".into()),
            Value::String("BadType".into()),
        );
        attrs.insert(
            Value::String("package".into()),
            Value::String("bad".into()),
        );
        attrs.insert(
            Value::String("definition".into()),
            Value::String(": not valid yaml mapping {{{{".into()),
        );

        state.objects.insert(
            ("node".into(), "bad-node-1".into()),
            make_node("bad-node-1", "meta/EntityType@1", attrs),
        );

        let result = Registry::from_state(&state);
        assert!(result.is_err(), "must return Err on bad definition YAML");
        let msg = result.err().unwrap();
        assert!(
            msg.contains("bad-node-1"),
            "error must name the offending node id, got: {msg}"
        );
    }
}
