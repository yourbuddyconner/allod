//! Lint Allod ontology packages against the specification.
//!
//! Validates the projection-form YAML under ontologies/ against the
//! rules of Part 1 (object model), Part 2 (schema), and Part 4 (policy):
//!
//!   ontology.yaml   type grammar, edge domains and ranges, cardinality,
//!                   inheritance, import declarations
//!   taxonomy.yaml   term uniqueness, parent resolution, acyclicity
//!   policy.yaml     selector grammar (all/any/not), requirement
//!                   vocabulary, role and region resolution
//!   examples/*.yaml instance envelopes per object kind, type and term
//!                   resolution, required attributes, hash prefixes
//!
//! Usage: allod-lint [ontologies-dir]
//!
//! Exit code 1 when any error is found. State hashes in imports are
//! placeholders until the reference implementation ships Appendix H
//! vectors, so hash values are checked for algorithm prefixes only.

use serde::Deserialize;
use serde_yaml::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const BASE_TYPES: &[&str] = &[
    "string", "int", "float", "decimal", "bool", "timestamp", "date",
    "duration", "bytes", "node-ref", "edge-ref", "document-ref",
    "external-ref",
];
const CARDINALITIES: &[&str] =
    &["many-to-many", "many-to-one", "one-to-many", "one-to-one"];
const POSTURES: &[&str] = &["open", "restricted"];
const SELECTOR_KEYS: &[&str] = &[
    "author_kind", "basis", "operation", "region", "repo", "substrate",
    "target_ref", "type",
];
const AUTHOR_KINDS: &[&str] = &["agent", "service", "user"];
const BASES: &[&str] = &["deterministic", "manual", "model-assisted"];
const REQUIREMENT_KEYS: &[&str] = &[
    "attestation_required", "authors", "classification_required",
    "reviewers", "review_window", "schema_valid", "substrate_checks",
];
const OPERATIONS: &[&str] = &[
    "create", "update", "delete", "resolve", "redact-document",
    "redact-operation", "define-type", "set-policy", "deprecate-term",
];
const DOC_KINDS: &[&str] = &[
    "changeset", "classification", "decision-record", "document", "edge",
    "node",
];
const VERDICTS: &[&str] = &["abstain", "approve", "reject"];
const STORAGE: &[&str] = &["external", "inline", "stored"];
const TERM_STATUS: &[&str] = &["active", "deprecated"];
const HASH_FIELDS: &[&str] = &[
    "content_hash", "hash", "policy_context", "rev", "schema_context",
    "state_hash",
];

fn get_str<'a>(doc: &'a Value, key: &str) -> Option<&'a str> {
    doc.get(key).and_then(Value::as_str)
}

fn has_algo_prefix(s: &str) -> bool {
    match s.find(':') {
        Some(i) if i > 0 => s[..i]
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
        _ => false,
    }
}

fn truncate_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn bare(reference: &str) -> &str {
    reference.split('@').next().unwrap_or(reference)
}

#[derive(Clone)]
struct Package {
    types: Value,
    edges: Value,
    imports: Vec<String>,
}

struct Taxonomy {
    terms: HashMap<String, Vec<String>>,
}

#[derive(Default)]
struct Linter {
    errors: Vec<String>,
    warnings: Vec<String>,
    packages: HashMap<String, Package>,
    taxonomies: HashMap<String, Taxonomy>,
    docs: Vec<(PathBuf, Value)>,
}

impl Linter {
    fn error(&mut self, path: &Path, where_: &str, msg: &str) {
        self.errors.push(format!("{}: {}: {}", path.display(), where_, msg));
    }

    fn warn(&mut self, path: &Path, where_: &str, msg: &str) {
        self.warnings
            .push(format!("{}: {}: {}", path.display(), where_, msg));
    }

    // ---------------- loading ----------------

    fn load_dir(&mut self, root: &Path) {
        let mut files = Vec::new();
        collect_yaml(root, &mut files);
        files.sort();
        for path in files {
            let text = match fs::read_to_string(&path) {
                Ok(text) => text,
                Err(err) => {
                    self.error(&path, "io", &err.to_string());
                    continue;
                }
            };
            for de in serde_yaml::Deserializer::from_str(&text) {
                match Value::deserialize(de) {
                    Ok(doc) if doc.is_mapping() => {
                        self.docs.push((path.clone(), doc))
                    }
                    Ok(_) => {}
                    Err(err) => {
                        self.error(&path, "yaml", &format!("parse failure: {err}"));
                        break;
                    }
                }
            }
        }
        // Register schemas first so references resolve regardless of order.
        let docs = self.docs.clone();
        for (path, doc) in &docs {
            if doc.get("ontology").is_some() {
                self.register_ontology(path, doc);
            } else if doc.get("taxonomy").is_some() {
                self.register_taxonomy(path, doc);
            }
        }
    }

    fn register_ontology(&mut self, path: &Path, doc: &Value) {
        let Some(name) = get_str(doc, "ontology") else {
            self.error(path, "ontology", "missing or invalid package name");
            return;
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
                imports,
            },
        );
    }

    fn register_taxonomy(&mut self, path: &Path, doc: &Value) {
        let Some(name) = get_str(doc, "taxonomy") else {
            self.error(path, "taxonomy", "missing or invalid taxonomy name");
            return;
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
    }

    // ---------------- resolution helpers ----------------

    /// Resolve `Type` or `pkg/Type`, returning (pkg, name, definition).
    fn resolve_type(
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

    fn resolve_edge_type(&self, reference: &str) -> Option<(String, Value)> {
        let name = bare(reference);
        let (pname, ename) = name.rsplit_once('/')?;
        let info = self.packages.get(pname)?;
        let def = info.edges.get(ename)?;
        Some((name.to_string(), def.clone()))
    }

    fn term_exists(&self, reference: &str) -> bool {
        let name = bare(reference);
        self.taxonomies
            .values()
            .any(|tax| tax.terms.contains_key(name))
    }

    /// Attribute map including inherited attributes.
    fn collected_attrs(
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
                for (aname, adef) in self.collected_attrs(&ppkg, &pname, seen) {
                    attrs.entry(aname).or_insert(adef);
                }
            }
        }
        attrs
    }

    // ---------------- type expression grammar ----------------

    fn check_type_expr(&mut self, path: &Path, where_: &str, expr: &Value) {
        let Some(expr) = expr.as_str() else {
            self.error(
                path,
                where_,
                &format!("attribute type must be a string, got {expr:?}"),
            );
            return;
        };
        self.check_type_str(path, where_, expr.trim());
    }

    fn check_type_str(&mut self, path: &Path, where_: &str, expr: &str) {
        if BASE_TYPES.contains(&expr) {
            return;
        }
        if let Some(inner) = expr
            .strip_prefix("list<")
            .and_then(|rest| rest.strip_suffix('>'))
        {
            self.check_type_str(path, where_, inner.trim());
            return;
        }
        if let Some(inner) = expr
            .strip_prefix("map<")
            .and_then(|rest| rest.strip_suffix('>'))
        {
            let mut depth = 0usize;
            let mut split = None;
            for (i, ch) in inner.char_indices() {
                match ch {
                    '<' => depth += 1,
                    '>' => depth = depth.saturating_sub(1),
                    ',' if depth == 0 => {
                        split = Some(i);
                        break;
                    }
                    _ => {}
                }
            }
            let Some(split) = split else {
                self.error(
                    path,
                    where_,
                    &format!("map type needs key and value: {expr}"),
                );
                return;
            };
            let key = inner[..split].trim();
            if key != "string" {
                self.error(path, where_, &format!("map keys must be string, got {key}"));
            }
            self.check_type_str(path, where_, inner[split + 1..].trim());
            return;
        }
        self.error(path, where_, &format!("unknown attribute type {expr:?}"));
    }

    // ---------------- schema checks ----------------

    fn check_ontology(&mut self, path: &Path, doc: &Value) {
        let pkg = get_str(doc, "ontology").unwrap_or("<unnamed>").to_string();
        let where_ = format!("ontology {pkg}");
        if doc.get("version").and_then(Value::as_i64).is_none_or(|v| v < 1) {
            self.error(path, &where_, "version must be an integer >= 1");
        }
        if let Some(imports) = doc.get("imports").and_then(Value::as_sequence) {
            for imp in imports {
                let Some(target) = get_str(imp, "ontology") else {
                    self.error(path, &where_, &format!("malformed import {imp:?}"));
                    continue;
                };
                if !self.packages.contains_key(target) {
                    self.error(
                        path,
                        &where_,
                        &format!("import {target:?} is not a loaded package"),
                    );
                }
                let hash = get_str(imp, "state_hash").unwrap_or("");
                if !has_algo_prefix(hash) {
                    self.error(
                        path,
                        &where_,
                        &format!(
                            "import {target:?} state_hash lacks an algorithm \
                             prefix (§1.7)"
                        ),
                    );
                }
            }
        }
        let imports: BTreeSet<String> = self
            .packages
            .get(&pkg)
            .map(|p| p.imports.iter().cloned().collect())
            .unwrap_or_default();
        let foreign_ok = |reference: &str| -> bool {
            match bare(reference).rsplit_once('/') {
                Some((target, _)) => target == pkg || imports.contains(target),
                None => true,
            }
        };

        let entity_types = doc.get("entity_types").cloned().unwrap_or(Value::Null);
        if let Some(map) = entity_types.as_mapping() {
            for (tname, tdef) in map {
                let Some(tname) = tname.as_str() else { continue };
                let twhere = format!("entity_types.{tname}");
                if let Some(attrs) = tdef.get("attributes").and_then(Value::as_mapping)
                {
                    for (aname, adef) in attrs {
                        let Some(aname) = aname.as_str() else { continue };
                        let awhere = format!("{twhere}.attributes.{aname}");
                        let Some(atype) = adef.get("type") else {
                            self.error(path, &awhere, "attribute needs a type");
                            continue;
                        };
                        self.check_type_expr(path, &awhere, atype);
                        if let Some(req) = adef.get("required") {
                            if !req.is_bool() {
                                self.error(path, &awhere, "required must be a bool");
                            }
                        }
                        if let Some(target) = adef.get("target") {
                            if atype.as_str() != Some("node-ref") {
                                self.error(
                                    path,
                                    &awhere,
                                    "target is only valid on node-ref",
                                );
                            } else if target.as_str().is_none_or(|t| {
                                self.resolve_type(t, Some(&pkg)).is_none()
                            }) {
                                self.error(
                                    path,
                                    &awhere,
                                    &format!("target {target:?} does not resolve"),
                                );
                            }
                        }
                    }
                }
                if let Some(parent) = get_str(tdef, "extends") {
                    match self.resolve_type(parent, Some(&pkg)) {
                        None => self.error(
                            path,
                            &twhere,
                            &format!("extends {parent:?} does not resolve"),
                        ),
                        Some((ppkg, pname, _)) if !foreign_ok(parent) => {
                            let _ = (ppkg, pname);
                            self.error(
                                path,
                                &twhere,
                                &format!(
                                    "extends {parent:?} but package is not imported"
                                ),
                            );
                        }
                        Some((ppkg, pname, _)) => {
                            let inherited = self.collected_attrs(
                                &ppkg,
                                &pname,
                                &mut BTreeSet::new(),
                            );
                            if let Some(attrs) =
                                tdef.get("attributes").and_then(Value::as_mapping)
                            {
                                for (aname, adef) in attrs {
                                    let Some(aname) = aname.as_str() else {
                                        continue;
                                    };
                                    if let Some(idef) = inherited.get(aname) {
                                        if adef.get("type") != idef.get("type") {
                                            self.error(
                                                path,
                                                &format!(
                                                    "{twhere}.attributes.{aname}"
                                                ),
                                                "redefines an inherited attribute \
                                                 with a different type (§2.3 \
                                                 forbids removal or widening)",
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        let edge_types = doc.get("edge_types").cloned().unwrap_or(Value::Null);
        if let Some(map) = edge_types.as_mapping() {
            for (ename, edef) in map {
                let Some(ename) = ename.as_str() else { continue };
                let ewhere = format!("edge_types.{ename}");
                let card = get_str(edef, "cardinality").unwrap_or("<missing>");
                if !CARDINALITIES.contains(&card) {
                    self.error(
                        path,
                        &ewhere,
                        &format!(
                            "cardinality {card:?} not in {CARDINALITIES:?} (§2.1)"
                        ),
                    );
                }
                for side in ["domain", "range"] {
                    let Some(val) = edef.get(side) else {
                        self.error(path, &ewhere, &format!("missing {side}"));
                        continue;
                    };
                    let refs: Vec<&str> = match val {
                        Value::Sequence(seq) => {
                            seq.iter().filter_map(Value::as_str).collect()
                        }
                        other => other.as_str().into_iter().collect(),
                    };
                    for reference in refs {
                        if self.resolve_type(reference, Some(&pkg)).is_none() {
                            self.error(
                                path,
                                &ewhere,
                                &format!("{side} {reference:?} does not resolve"),
                            );
                        } else if !foreign_ok(reference) {
                            self.error(
                                path,
                                &ewhere,
                                &format!(
                                    "{side} {reference:?} but package is not imported"
                                ),
                            );
                        }
                    }
                }
                if let Some(attrs) = edef.get("attributes").and_then(Value::as_mapping)
                {
                    for (aname, adef) in attrs {
                        let Some(aname) = aname.as_str() else { continue };
                        let awhere = format!("{ewhere}.attributes.{aname}");
                        match adef.get("type") {
                            Some(atype) => self.check_type_expr(path, &awhere, atype),
                            None => self.error(path, &awhere, "attribute needs a type"),
                        }
                    }
                }
            }
        }

        if let Some(rules) = doc.get("validation_rules").and_then(Value::as_sequence) {
            for rule in rules {
                if get_str(rule, "name").is_none() || get_str(rule, "rule").is_none() {
                    self.error(
                        path,
                        "validation_rules",
                        &format!("rules need name and rule: {rule:?}"),
                    );
                }
            }
        }
    }

    fn check_taxonomy(&mut self, path: &Path, doc: &Value) {
        let name = get_str(doc, "taxonomy").unwrap_or("<unnamed>").to_string();
        let where_ = format!("taxonomy {name}");
        if doc.get("version").and_then(Value::as_i64).is_none_or(|v| v < 1) {
            self.error(path, &where_, "version must be an integer >= 1");
        }
        let mut seen = BTreeSet::new();
        let mut local: HashMap<String, Vec<String>> = HashMap::new();
        if let Some(terms) = doc.get("terms").and_then(Value::as_sequence) {
            for term in terms {
                let Some(tname) = get_str(term, "name") else {
                    self.error(path, &where_, &format!("malformed term {term:?}"));
                    continue;
                };
                if seen.contains(tname) {
                    self.error(path, &where_, &format!("duplicate term {tname:?}"));
                }
                seen.insert(tname.to_string());
                if let Some(status) = get_str(term, "status") {
                    if !TERM_STATUS.contains(&status) {
                        self.error(
                            path,
                            &format!("{where_}.{tname}"),
                            &format!("status {status:?} not in {TERM_STATUS:?}"),
                        );
                    }
                }
                let parents: Vec<String> = term
                    .get("parents")
                    .and_then(Value::as_sequence)
                    .map(|p| {
                        p.iter()
                            .filter_map(Value::as_str)
                            .map(String::from)
                            .collect()
                    })
                    .unwrap_or_default();
                for parent in &parents {
                    if !seen.contains(parent) && !self.term_exists(parent) {
                        self.error(
                            path,
                            &format!("{where_}.{tname}"),
                            &format!("parent {parent:?} does not resolve"),
                        );
                    }
                }
                local.insert(tname.to_string(), parents);
            }
        }
        // Acyclicity (§2.2): DFS over this taxonomy's parent links.
        let mut color: HashMap<String, u8> = HashMap::new();
        let names: Vec<String> = local.keys().cloned().collect();
        for term in names {
            if color.get(&term).copied().unwrap_or(0) == 0 {
                self.visit_term(path, &where_, &local, &term, &mut Vec::new(), &mut color);
            }
        }
    }

    fn visit_term(
        &mut self,
        path: &Path,
        where_: &str,
        local: &HashMap<String, Vec<String>>,
        node: &str,
        stack: &mut Vec<String>,
        color: &mut HashMap<String, u8>,
    ) {
        color.insert(node.to_string(), 1);
        stack.push(node.to_string());
        for parent in local.get(node).cloned().unwrap_or_default() {
            if !local.contains_key(&parent) {
                continue;
            }
            match color.get(&parent).copied().unwrap_or(0) {
                1 => {
                    let mut chain = stack.clone();
                    chain.push(parent.clone());
                    self.error(
                        path,
                        where_,
                        &format!(
                            "cycle through {} (the structure must be a DAG, §2.2)",
                            chain.join(" -> ")
                        ),
                    );
                }
                0 => self.visit_term(path, where_, local, &parent, stack, color),
                _ => {}
            }
        }
        stack.pop();
        color.insert(node.to_string(), 2);
    }

    // ---------------- policy checks ----------------

    fn check_selector(&mut self, path: &Path, where_: &str, sel: &Value) {
        let Some(map) = sel.as_mapping() else {
            self.error(path, where_, &format!("selector must be a map, got {sel:?}"));
            return;
        };
        for combinator in ["all", "any"] {
            if let Some(subs) = sel.get(combinator) {
                if let Some(seq) = subs.as_sequence() {
                    for (i, sub) in seq.iter().enumerate() {
                        self.check_selector(
                            path,
                            &format!("{where_}.{combinator}[{i}]"),
                            sub,
                        );
                    }
                }
                if map.len() > 1 {
                    self.error(
                        path,
                        where_,
                        &format!("{combinator} does not mix with other keys"),
                    );
                }
                return;
            }
        }
        if let Some(sub) = sel.get("not") {
            self.check_selector(path, &format!("{where_}.not"), sub);
            if map.len() > 1 {
                self.error(path, where_, "not does not mix with other keys");
            }
            return;
        }
        let unknown: Vec<String> = map
            .keys()
            .filter_map(Value::as_str)
            .filter(|k| !SELECTOR_KEYS.contains(k))
            .map(String::from)
            .collect();
        if !unknown.is_empty() {
            self.error(
                path,
                where_,
                &format!("unknown selector keys {unknown:?} (§4.1)"),
            );
        }
        if let Some(kind) = get_str(sel, "author_kind") {
            if !AUTHOR_KINDS.contains(&kind) {
                self.error(
                    path,
                    where_,
                    &format!("author_kind {kind:?} not in {AUTHOR_KINDS:?}"),
                );
            }
        }
        if let Some(basis) = get_str(sel, "basis") {
            if !BASES.contains(&basis) {
                self.error(path, where_, &format!("basis {basis:?} not in {BASES:?}"));
            }
        }
        if let Some(region) = get_str(sel, "region") {
            if !self.term_exists(region) {
                self.error(
                    path,
                    where_,
                    &format!("region {region:?} resolves to no taxonomy term"),
                );
            }
        }
        if let Some(tref) = get_str(sel, "type") {
            if self.resolve_type(tref, None).is_none() {
                self.error(path, where_, &format!("type {tref:?} does not resolve"));
            }
        }
        if let Some(ops) = sel.get("operation") {
            let ops: Vec<&str> = match ops {
                Value::Sequence(seq) => seq.iter().filter_map(Value::as_str).collect(),
                other => other.as_str().into_iter().collect(),
            };
            for op in ops {
                if !OPERATIONS.contains(&op) {
                    self.error(path, where_, &format!("unknown operation {op:?}"));
                }
            }
        }
    }

    fn check_reviewers(
        &mut self,
        path: &Path,
        where_: &str,
        val: &Value,
        roles: &BTreeSet<String>,
    ) {
        let entries: Vec<&Value> = match val {
            Value::Sequence(seq) => seq.iter().collect(),
            other => vec![other],
        };
        for entry in entries {
            let Some(role) = get_str(entry, "role") else {
                self.error(
                    path,
                    where_,
                    &format!("reviewers entries need a role: {entry:?}"),
                );
                continue;
            };
            if !roles.contains(role) {
                self.error(
                    path,
                    where_,
                    &format!("role {role:?} is not declared in roles"),
                );
            }
            if let Some(quorum) = entry.get("quorum") {
                if quorum.as_i64().is_none_or(|q| q < 1) {
                    self.error(path, where_, "quorum must be an integer >= 1");
                }
            }
        }
    }

    fn check_policy(&mut self, path: &Path, doc: &Value) {
        let name = get_str(doc, "policy").unwrap_or("<unnamed>").to_string();
        let where_ = format!("policy {name}");
        if doc.get("version").and_then(Value::as_i64).is_none_or(|v| v < 1) {
            self.error(path, &where_, "version must be an integer >= 1");
        }
        let posture = get_str(doc, "default_posture").unwrap_or("<missing>");
        if !POSTURES.contains(&posture) {
            self.error(
                path,
                &where_,
                &format!("default_posture {posture:?} not in {POSTURES:?} (§4.1)"),
            );
        }
        let roles: BTreeSet<String> = doc
            .get("roles")
            .and_then(Value::as_mapping)
            .map(|map| {
                map.keys()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        let mut seen_rules = BTreeSet::new();
        if let Some(rules) = doc.get("rules").and_then(Value::as_sequence) {
            for rule in rules {
                let rname = get_str(rule, "name").unwrap_or("<unnamed>").to_string();
                let rwhere = format!("{where_}.rules.{rname}");
                if seen_rules.contains(&rname) {
                    self.error(path, &rwhere, "duplicate rule name");
                }
                seen_rules.insert(rname);
                let (Some(select), Some(require)) =
                    (rule.get("select"), rule.get("require"))
                else {
                    self.error(path, &rwhere, "rules need select and require");
                    continue;
                };
                self.check_selector(path, &format!("{rwhere}.select"), select);
                let Some(rmap) = require.as_mapping() else {
                    self.error(path, &rwhere, "require must be a map");
                    continue;
                };
                let unknown: Vec<String> = rmap
                    .keys()
                    .filter_map(Value::as_str)
                    .filter(|k| !REQUIREMENT_KEYS.contains(k))
                    .map(String::from)
                    .collect();
                if !unknown.is_empty() {
                    self.error(
                        path,
                        &rwhere,
                        &format!("unknown requirement keys {unknown:?} (§4.2)"),
                    );
                }
                if let Some(reviewers) = require.get("reviewers") {
                    self.check_reviewers(
                        path,
                        &format!("{rwhere}.require.reviewers"),
                        reviewers,
                        &roles,
                    );
                }
                if let Some(window) = require.get("review_window") {
                    let ok_shape = window.as_mapping().is_some_and(|map| {
                        map.keys().filter_map(Value::as_str).all(|k| {
                            matches!(k, "min" | "max" | "on_expiry")
                        })
                    });
                    if !ok_shape {
                        self.error(
                            path,
                            &rwhere,
                            &format!("malformed review_window {window:?}"),
                        );
                    } else if let Some(expiry) = get_str(window, "on_expiry") {
                        if expiry != "reject" {
                            self.error(
                                path,
                                &rwhere,
                                &format!("on_expiry {expiry:?} unsupported"),
                            );
                        }
                    }
                }
            }
        }
    }

    // ---------------- instance checks ----------------

    fn check_hashes(&mut self, path: &Path, where_: &str, doc: &Value) {
        for field in HASH_FIELDS {
            if let Some(val) = get_str(doc, field) {
                if !has_algo_prefix(val) {
                    self.error(
                        path,
                        where_,
                        &format!("{field} {val:?} lacks an algorithm prefix (§1.7)"),
                    );
                }
            }
        }
    }

    fn check_node_payload(&mut self, path: &Path, where_: &str, doc: &Value) {
        let reference = get_str(doc, "type").unwrap_or("<missing>");
        let Some((pkg, tname, _)) = self.resolve_type(reference, None) else {
            self.error(
                path,
                where_,
                &format!("node type {reference:?} does not resolve"),
            );
            return;
        };
        if !reference.contains('@') {
            self.error(
                path,
                where_,
                &format!("type ref {reference:?} lacks a version (§1.2)"),
            );
        }
        let attrs = self.collected_attrs(&pkg, &tname, &mut BTreeSet::new());
        let given: BTreeSet<String> = doc
            .get("attributes")
            .and_then(Value::as_mapping)
            .map(|map| {
                map.keys()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        for (aname, adef) in &attrs {
            let required = adef.get("required").and_then(Value::as_bool) == Some(true);
            if required && !given.contains(aname) {
                self.error(
                    path,
                    where_,
                    &format!("missing required attribute {aname:?} of {pkg}/{tname}"),
                );
            }
        }
        for aname in &given {
            if !attrs.contains_key(aname) {
                self.error(
                    path,
                    where_,
                    &format!("attribute {aname:?} is not declared on {pkg}/{tname}"),
                );
            }
        }
    }

    fn check_edge_payload(&mut self, path: &Path, where_: &str, doc: &Value) {
        let reference = get_str(doc, "type").unwrap_or("<missing>");
        let Some((ename, edef)) = self.resolve_edge_type(reference) else {
            self.error(
                path,
                where_,
                &format!("edge type {reference:?} does not resolve"),
            );
            return;
        };
        let _ = ename;
        if !reference.contains('@') {
            self.error(
                path,
                where_,
                &format!("type ref {reference:?} lacks a version (§1.4)"),
            );
        }
        for side in ["from", "to"] {
            if doc.get(side).is_none() {
                self.error(path, where_, &format!("edge missing {side}"));
            }
        }
        let declared: BTreeSet<String> = edef
            .get("attributes")
            .and_then(Value::as_mapping)
            .map(|map| {
                map.keys()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        if let Some(given) = doc.get("attributes").and_then(Value::as_mapping) {
            for aname in given.keys().filter_map(Value::as_str) {
                if !declared.contains(aname) {
                    self.error(
                        path,
                        where_,
                        &format!(
                            "attribute {aname:?} is not declared on edge type \
                             {reference}"
                        ),
                    );
                }
            }
        }
    }

    fn check_classification_payload(
        &mut self,
        path: &Path,
        where_: &str,
        doc: &Value,
        in_op: bool,
    ) {
        for field in ["subject", "term"] {
            if doc.get(field).is_none() {
                self.error(path, where_, &format!("classification missing {field}"));
            }
        }
        if let Some(term) = get_str(doc, "term") {
            if !self.term_exists(term) {
                self.error(
                    path,
                    where_,
                    &format!("term {term:?} resolves to no taxonomy term"),
                );
            }
        }
        if !in_op {
            if doc.get("asserted_by").is_none() {
                self.error(path, where_, "classification missing asserted_by (§1.6)");
            }
            let basis = get_str(doc, "basis").unwrap_or("<missing>");
            if !BASES.contains(&basis) {
                self.error(
                    path,
                    where_,
                    &format!("basis {basis:?} not in {BASES:?} (§1.6)"),
                );
            }
        }
    }

    fn check_provenance(&mut self, path: &Path, where_: &str, prov: &Value) {
        if !prov.is_mapping() {
            self.error(path, where_, "provenance must be a map (§5.1)");
            return;
        }
        let method = get_str(prov, "method").unwrap_or("<missing>");
        if !BASES.contains(&method) {
            self.error(
                path,
                where_,
                &format!("lineage method {method:?} not in {BASES:?}"),
            );
        }
        if matches!(method, "deterministic" | "model-assisted")
            && get_str(prov, "tool").is_none()
        {
            self.error(path, where_, "non-manual lineage must name a tool (§5.1)");
        }
    }

    fn check_instance(&mut self, path: &Path, doc: &Value) {
        let kind = get_str(doc, "kind").unwrap_or("<missing>").to_string();
        let ident = get_str(doc, "id")
            .or_else(|| get_str(doc, "hash"))
            .or_else(|| get_str(doc, "subject"))
            .unwrap_or("?");
        let where_ = format!("{kind} {}", truncate_chars(ident, 12));
        if !DOC_KINDS.contains(&kind.as_str()) {
            self.error(path, &where_, &format!("unknown kind {kind:?}"));
            return;
        }
        self.check_hashes(path, &where_, doc);
        if let Some(prov) = doc.get("provenance") {
            self.check_provenance(path, &format!("{where_}.provenance"), prov);
        }
        match kind.as_str() {
            "node" => self.check_node_payload(path, &where_, doc),
            "edge" => self.check_edge_payload(path, &where_, doc),
            "classification" => {
                self.check_classification_payload(path, &where_, doc, false)
            }
            "document" => {
                for field in ["content_hash", "media_type", "storage"] {
                    if doc.get(field).is_none() {
                        self.error(
                            path,
                            &where_,
                            &format!("document missing {field} (§1.5)"),
                        );
                    }
                }
                let storage = get_str(doc, "storage").unwrap_or("<missing>");
                if !STORAGE.contains(&storage) {
                    self.error(
                        path,
                        &where_,
                        &format!("storage {storage:?} not in {STORAGE:?}"),
                    );
                }
            }
            "changeset" => {
                for field in [
                    "hash", "parents", "author", "timestamp", "operations",
                    "schema_context", "signature",
                ] {
                    if doc.get(field).is_none() {
                        self.error(
                            path,
                            &where_,
                            &format!("changeset missing {field} (§3.2.1)"),
                        );
                    }
                }
                if let Some(ops) = doc.get("operations").and_then(Value::as_sequence) {
                    for (i, op) in ops.iter().enumerate() {
                        self.check_operation(
                            path,
                            &format!("{where_}.operations[{i}]"),
                            op,
                        );
                    }
                }
            }
            "decision-record" => {
                for field in [
                    "subject", "policy_context", "verdict", "deciders", "timestamp",
                ] {
                    if doc.get(field).is_none() {
                        self.error(
                            path,
                            &where_,
                            &format!("decision record missing {field} (§4.3)"),
                        );
                    }
                }
                let verdict = get_str(doc, "verdict").unwrap_or("<missing>");
                if !VERDICTS.contains(&verdict) {
                    self.error(
                        path,
                        &where_,
                        &format!("verdict {verdict:?} not in {VERDICTS:?}"),
                    );
                }
            }
            _ => {}
        }
    }

    fn check_operation(&mut self, path: &Path, where_: &str, op: &Value) {
        let Some(map) = op.as_mapping() else {
            self.error(
                path,
                where_,
                &format!("operations carry exactly one verb: {op:?}"),
            );
            return;
        };
        if map.len() != 1 {
            self.error(
                path,
                where_,
                &format!("operations carry exactly one verb: {op:?}"),
            );
            return;
        }
        let (verb, payload) = map.iter().next().unwrap();
        let verb = verb.as_str().unwrap_or("<invalid>");
        if !OPERATIONS.contains(&verb) {
            self.error(path, where_, &format!("unknown operation {verb:?}"));
            return;
        }
        if !payload.is_mapping() {
            self.error(path, where_, "operation payload must be a map");
            return;
        }
        if verb == "create" {
            match get_str(payload, "kind") {
                Some("node") => self.check_node_payload(path, where_, payload),
                Some("edge") => self.check_edge_payload(path, where_, payload),
                Some("classification") => {
                    self.check_classification_payload(path, where_, payload, true)
                }
                _ => {}
            }
        }
    }

    // ---------------- driver ----------------

    fn run(&mut self, root: &Path) -> u8 {
        self.load_dir(root);
        let docs = self.docs.clone();
        for (path, doc) in &docs {
            if doc.get("ontology").is_some() {
                self.check_ontology(path, doc);
            } else if doc.get("taxonomy").is_some() {
                self.check_taxonomy(path, doc);
            } else if doc.get("policy").is_some() {
                self.check_policy(path, doc);
            } else if doc.get("kind").is_some() {
                self.check_instance(path, doc);
            } else {
                self.warn(path, "document", "unrecognized document shape");
            }
        }
        for line in &self.errors {
            println!("ERROR {line}");
        }
        for line in &self.warnings {
            println!("WARN  {line}");
        }
        println!(
            "\nchecked {} packages, {} taxonomies: {} errors, {} warnings",
            self.packages.len(),
            self.taxonomies.len(),
            self.errors.len(),
            self.warnings.len()
        );
        u8::from(!self.errors.is_empty())
    }
}

fn collect_yaml(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect_yaml(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("yaml") {
            out.push(path);
        }
    }
}

fn main() -> ExitCode {
    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("ontologies"));
    if !root.is_dir() {
        eprintln!("no such directory: {}", root.display());
        return ExitCode::from(2);
    }
    ExitCode::from(Linter::default().run(&root))
}
