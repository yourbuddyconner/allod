//! Package and taxonomy registries with reference resolution.

use crate::{bare, get_str};
use serde_yaml::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// A loaded ontology package in projection form.
#[derive(Clone)]
pub struct Package {
    pub types: Value,
    pub edges: Value,
    pub structs: Value,
    pub imports: Vec<String>,
}

/// A loaded taxonomy: term name to parent terms.
pub struct Taxonomy {
    pub terms: HashMap<String, Vec<String>>,
}

/// Everything loaded from a set of packages, with resolution over it.
#[derive(Default)]
pub struct Registry {
    pub packages: HashMap<String, Package>,
    pub taxonomies: HashMap<String, Taxonomy>,
}

impl Registry {
    /// Register an ontology document. Returns false when the package
    /// name is missing or invalid.
    pub fn register_ontology(&mut self, doc: &Value) -> bool {
        let Some(name) = get_str(doc, "ontology") else {
            return false;
        };
        let imports = doc
            .get("imports")
            .and_then(Value::as_sequence)
            .map(|seq| {
                seq.iter()
                    .filter_map(|imp| get_str(imp, "ontology"))
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        self.packages.insert(
            name.to_string(),
            Package {
                types: doc.get("entity_types").cloned().unwrap_or(Value::Null),
                edges: doc.get("edge_types").cloned().unwrap_or(Value::Null),
                structs: doc.get("structs").cloned().unwrap_or(Value::Null),
                imports,
            },
        );
        true
    }

    /// Register a taxonomy document. Returns false when the taxonomy
    /// name is missing or invalid.
    pub fn register_taxonomy(&mut self, doc: &Value) -> bool {
        let Some(name) = get_str(doc, "taxonomy") else {
            return false;
        };
        let mut terms = HashMap::new();
        if let Some(seq) = doc.get("terms").and_then(Value::as_sequence) {
            for term in seq {
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
                    terms.insert(tname.to_string(), parents);
                }
            }
        }
        self.taxonomies.insert(name.to_string(), Taxonomy { terms });
        true
    }

    /// Resolve `Type` or `pkg/Type`, returning (pkg, name, definition).
    pub fn resolve_type(
        &self,
        reference: &str,
        pkg: Option<&str>,
    ) -> Option<(String, String, Value)> {
        let name = bare(reference);
        if let Some((pname, tname)) = name.rsplit_once('/') {
            let info = self.packages.get(pname)?;
            let def = info.types.get(tname)?;
            return Some((pname.to_string(), tname.to_string(), def.clone()));
        }
        let pname = pkg?;
        let info = self.packages.get(pname)?;
        let def = info.types.get(name)?;
        Some((pname.to_string(), name.to_string(), def.clone()))
    }

    /// Resolve `pkg/edge`, returning (bare name, definition).
    pub fn resolve_edge_type(&self, reference: &str) -> Option<(String, Value)> {
        let name = bare(reference);
        let (pname, ename) = name.rsplit_once('/')?;
        let info = self.packages.get(pname)?;
        let def = info.edges.get(ename)?;
        Some((name.to_string(), def.clone()))
    }

    /// Resolve a struct by its graph-global name (§2.1), returning
    /// (declaring package, definition). Names are unique across
    /// packages; the linter reports collisions.
    pub fn resolve_struct(&self, name: &str) -> Option<(String, Value)> {
        for (pname, package) in &self.packages {
            if let Some(def) = package.structs.get(name) {
                return Some((pname.clone(), def.clone()));
            }
        }
        None
    }

    /// Every declared struct name across loaded packages.
    pub fn struct_names(&self) -> BTreeSet<String> {
        self.packages
            .values()
            .filter_map(|p| p.structs.as_mapping())
            .flat_map(|map| {
                map.keys().filter_map(Value::as_str).map(String::from)
            })
            .collect()
    }

    /// True when any loaded taxonomy defines the term.
    pub fn term_exists(&self, reference: &str) -> bool {
        let name = bare(reference);
        self.taxonomies
            .values()
            .any(|tax| tax.terms.contains_key(name))
    }

    /// Attribute map for a type, including inherited attributes (§2.3).
    pub fn collected_attrs(&self, pkg: &str, tname: &str) -> BTreeMap<String, Value> {
        let mut seen = BTreeSet::new();
        self.collect_attrs_inner(pkg, tname, &mut seen)
    }

    fn collect_attrs_inner(
        &self,
        pkg: &str,
        tname: &str,
        seen: &mut BTreeSet<(String, String)>,
    ) -> BTreeMap<String, Value> {
        let key = (pkg.to_string(), tname.to_string());
        if seen.contains(&key) {
            return BTreeMap::new();
        }
        seen.insert(key);
        let Some(info) = self.packages.get(pkg) else {
            return BTreeMap::new();
        };
        let Some(tdef) = info.types.get(tname).cloned() else {
            return BTreeMap::new();
        };
        let mut attrs = BTreeMap::new();
        if let Some(map) = tdef.get("attributes").and_then(Value::as_mapping) {
            for (aname, adef) in map {
                if let Some(aname) = aname.as_str() {
                    attrs.insert(aname.to_string(), adef.clone());
                }
            }
        }
        if let Some(parent) = get_str(&tdef, "extends") {
            if let Some((ppkg, pname, _)) = self.resolve_type(parent, Some(pkg)) {
                for (aname, adef) in self.collect_attrs_inner(&ppkg, &pname, seen) {
                    attrs.entry(aname).or_insert(adef);
                }
            }
        }
        attrs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> Registry {
        let core: Value = serde_yaml::from_str(
            r#"
ontology: core
version: 1
entity_types:
  Person:
    attributes:
      name: { type: string, required: true }
"#,
        )
        .unwrap();
        let corp: Value = serde_yaml::from_str(
            r#"
ontology: corp
version: 1
imports: [ { ontology: core, state_hash: "sha256:x" } ]
entity_types:
  Employee:
    extends: core/Person
    attributes:
      title: { type: string, required: true }
"#,
        )
        .unwrap();
        let mut reg = Registry::default();
        assert!(reg.register_ontology(&core));
        assert!(reg.register_ontology(&corp));
        reg
    }

    #[test]
    fn resolves_across_packages() {
        let reg = registry();
        assert!(reg.resolve_type("corp/Employee@1", None).is_some());
        assert!(reg.resolve_type("corp/Ghost", None).is_none());
    }

    #[test]
    fn inherits_attributes() {
        let reg = registry();
        let attrs = reg.collected_attrs("corp", "Employee");
        assert!(attrs.contains_key("title"));
        assert!(attrs.contains_key("name"), "inherited from core/Person");
    }
}
