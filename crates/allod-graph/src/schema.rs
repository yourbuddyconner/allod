//! Registry introspection: project the loaded ontology into a typed description.

use allod_core::bare;
use allod_core::get_str;
use allod_core::meta::is_meta_type;
use allod_core::store::Graph;
use serde_yaml::Value;
use std::collections::HashMap;

use crate::AllodError;

/// A single attribute on an entity type.
#[derive(Debug, serde::Serialize)]
pub struct AttributeView {
    pub name: String,
    pub type_expr: String,
    pub required: bool,
}

/// A single entity type (with inherited attributes included).
#[derive(Debug, serde::Serialize)]
pub struct EntityTypeView {
    pub name: String,
    pub version: Option<u64>,
    pub extends: Option<String>,
    pub attributes: Vec<AttributeView>,
}

/// A single edge type.
#[derive(Debug, serde::Serialize)]
pub struct EdgeTypeView {
    pub name: String,
    pub version: Option<u64>,
    pub domain: Vec<String>,
    pub range: Vec<String>,
    pub cardinality: Option<String>,
}

/// A taxonomy term.
#[derive(Debug, serde::Serialize)]
pub struct TermView {
    pub name: String,
    pub version: Option<u64>,
    pub parents: Vec<String>,
    pub status: Option<String>,
}

/// The complete description of the schema loaded in a graph.
#[derive(Debug, serde::Serialize)]
pub struct SchemaDescription {
    pub entity_types: Vec<EntityTypeView>,
    pub edge_types: Vec<EdgeTypeView>,
    pub terms: Vec<TermView>,
}

/// Produce a `SchemaDescription` by walking the registry of `graph`.
///
/// `version` and `status` fields on `EntityTypeView` and `TermView` are populated
/// from the live meta-typed nodes in the folded state (closing sub-project-1 ledger
/// note: previously these were always `None`).
pub fn describe(graph: &Graph) -> Result<SchemaDescription, AllodError> {
    let reg = graph.registry().map_err(AllodError::from)?;
    let state = graph.fold().map_err(AllodError::from)?;

    // Build lookup maps from meta-node state: (package, name) → version for EntityType/EdgeType/Struct,
    // and (term_name) → (version, status) for TaxonomyTerm.
    let mut entity_type_version: HashMap<(String, String), Option<u64>> = HashMap::new();
    let mut term_meta: HashMap<String, (Option<u64>, Option<String>)> = HashMap::new();

    for ((kind, _), obj) in &state.objects {
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
            "meta/EntityType" | "meta/EdgeType" | "meta/Struct" => {
                let name = match get_attr("name") { Some(n) => n, None => continue };
                let package = match get_attr("package") { Some(p) => p, None => continue };
                let version = attrs.and_then(|a| a.get("version")).and_then(Value::as_u64);
                entity_type_version.insert((package.to_string(), name.to_string()), version);
            }
            "meta/TaxonomyTerm" => {
                let name = match get_attr("name") { Some(n) => n, None => continue };
                let version = attrs.and_then(|a| a.get("version")).and_then(Value::as_u64);
                let status = get_attr("status").map(String::from);
                term_meta.insert(name.to_string(), (version, status));
            }
            _ => {}
        }
    }

    let mut entity_types = Vec::new();
    let mut edge_types = Vec::new();
    let mut terms = Vec::new();

    // Collect packages in sorted order for deterministic output
    let mut pkg_names: Vec<&String> = reg.packages.keys().collect();
    pkg_names.sort();

    for pkg_name in pkg_names {
        let package = &reg.packages[pkg_name];

        // FIXME: redundant folds — registry() already folds for package lookup above.
        // Entity types
        if let Some(map) = package.types.as_mapping() {
            let mut type_names: Vec<&str> = map
                .keys()
                .filter_map(Value::as_str)
                .collect();
            type_names.sort();
            for tname in type_names {
                let tdef = &map[tname];
                let extends = tdef
                    .get("extends")
                    .and_then(Value::as_str)
                    .map(String::from);

                // Version from meta node state (closes sub-project-1 ledger note).
                let version = entity_type_version
                    .get(&(pkg_name.clone(), tname.to_string()))
                    .copied()
                    .flatten();

                // Collect inherited+own attributes from registry
                let collected = reg.collected_attrs(pkg_name, tname);
                let mut attrs: Vec<AttributeView> = collected
                    .into_iter()
                    .map(|(aname, adef)| {
                        let type_expr = adef
                            .get("type")
                            .and_then(Value::as_str)
                            .unwrap_or("string")
                            .to_string();
                        let required = adef
                            .get("required")
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                        AttributeView { name: aname, type_expr, required }
                    })
                    .collect();
                attrs.sort_by(|a, b| a.name.cmp(&b.name));

                entity_types.push(EntityTypeView {
                    name: format!("{pkg_name}/{tname}"),
                    version,
                    extends,
                    attributes: attrs,
                });
            }
        }

        // Edge types
        if let Some(map) = package.edges.as_mapping() {
            let mut edge_names: Vec<&str> = map
                .keys()
                .filter_map(Value::as_str)
                .collect();
            edge_names.sort();
            for ename in edge_names {
                let edef = &map[ename];
                let version = entity_type_version
                    .get(&(pkg_name.clone(), ename.to_string()))
                    .copied()
                    .flatten();
                let domain = value_to_str_list(edef.get("domain"));
                let range = value_to_str_list(edef.get("range"));
                let cardinality = edef
                    .get("cardinality")
                    .and_then(Value::as_str)
                    .map(String::from);
                edge_types.push(EdgeTypeView {
                    name: format!("{pkg_name}/{ename}"),
                    version,
                    domain,
                    range,
                    cardinality,
                });
            }
        }
    }

    // Collect taxonomies in sorted order
    let mut tax_names: Vec<&String> = reg.taxonomies.keys().collect();
    tax_names.sort();

    for tax_name in tax_names {
        let taxonomy = &reg.taxonomies[tax_name];
        let mut tterm_names: Vec<&String> = taxonomy.terms.keys().collect();
        tterm_names.sort();
        for tname in tterm_names {
            let parents = taxonomy.terms[tname].clone();
            // version and status from meta node state (closes sub-project-1 ledger note).
            let (version, status) = term_meta
                .get(tname.as_str())
                .cloned()
                .unwrap_or((None, None));
            terms.push(TermView {
                name: tname.clone(),
                version,
                parents,
                status,
            });
        }
    }

    Ok(SchemaDescription { entity_types, edge_types, terms })
}

/// Convert a YAML value (string or sequence of strings) to Vec<String>.
fn value_to_str_list(val: Option<&Value>) -> Vec<String> {
    match val {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Sequence(seq)) => seq
            .iter()
            .filter_map(Value::as_str)
            .map(String::from)
            .collect(),
        _ => vec![],
    }
}
