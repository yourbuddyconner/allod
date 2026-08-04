//! Registry introspection: project the loaded ontology into a typed description.

use allod_core::store::Graph;
use serde_yaml::Value;

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
    pub domain: Vec<String>,
    pub range: Vec<String>,
    pub cardinality: Option<String>,
}

/// A taxonomy term.
#[derive(Debug, serde::Serialize)]
pub struct TermView {
    pub name: String,
    pub parents: Vec<String>,
}

/// The complete description of the schema loaded in a graph.
#[derive(Debug, serde::Serialize)]
pub struct SchemaDescription {
    pub entity_types: Vec<EntityTypeView>,
    pub edge_types: Vec<EdgeTypeView>,
    pub terms: Vec<TermView>,
}

/// Produce a `SchemaDescription` by walking the registry of `graph`.
pub fn describe(graph: &Graph) -> Result<SchemaDescription, AllodError> {
    let reg = graph.registry().map_err(AllodError::from)?;

    let mut entity_types = Vec::new();
    let mut edge_types = Vec::new();
    let mut terms = Vec::new();

    // Collect packages in sorted order for deterministic output
    let mut pkg_names: Vec<&String> = reg.packages.keys().collect();
    pkg_names.sort();

    for pkg_name in pkg_names {
        let package = &reg.packages[pkg_name];

        // Entity types
        if let Some(map) = package.types.as_mapping() {
            let mut type_names: Vec<&str> = map
                .keys()
                .filter_map(Value::as_str)
                .collect();
            type_names.sort();
            for tname in type_names {
                let tdef = &map[tname];
                let version = tdef.get("version").and_then(Value::as_u64);
                let extends = tdef
                    .get("extends")
                    .and_then(Value::as_str)
                    .map(String::from);

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
                let domain = value_to_str_list(edef.get("domain"));
                let range = value_to_str_list(edef.get("range"));
                let cardinality = edef
                    .get("cardinality")
                    .and_then(Value::as_str)
                    .map(String::from);
                edge_types.push(EdgeTypeView {
                    name: format!("{pkg_name}/{ename}"),
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
            terms.push(TermView {
                name: tname.clone(),
                parents,
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
