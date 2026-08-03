//! Lint Allod ontology packages against the specification.
//!
//! The domain model (vocabulary, registries, resolution, type grammar,
//! loading) lives in allod-core. This binary runs the checks:
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

use allod_core::vocab::{
    AUTHOR_KINDS, BASES, CARDINALITIES, DOC_KINDS, HASH_FIELDS, OPERATIONS,
    POSTURES, REQUIREMENT_KEYS, SELECTOR_KEYS, STORAGE, TERM_STATUS, VERDICTS,
};
use allod_core::{bare, get_str, has_algo_prefix, type_expr_errors, Registry};
use serde_yaml::Value;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn truncate_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

#[derive(Default)]
struct Linter {
    errors: Vec<String>,
    warnings: Vec<String>,
    registry: Registry,
    structs: BTreeSet<String>,
    ontology_docs: HashMap<String, Value>,
}

impl Linter {
    fn error(&mut self, path: &Path, where_: &str, msg: &str) {
        self.errors.push(format!("{}: {}: {}", path.display(), where_, msg));
    }

    fn warn(&mut self, path: &Path, where_: &str, msg: &str) {
        self.warnings
            .push(format!("{}: {}: {}", path.display(), where_, msg));
    }

    fn check_type_expr(&mut self, path: &Path, where_: &str, expr: &Value) {
        match expr.as_str() {
            None => self.error(
                path,
                where_,
                &format!("attribute type must be a string, got {expr:?}"),
            ),
            Some(expr) => {
                let msgs = type_expr_errors(expr, &self.structs);
                for msg in msgs {
                    self.error(path, where_, &msg);
                }
            }
        }
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
                if !self.registry.packages.contains_key(target) {
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
                    continue;
                }
                // Verify the declared hash against the imported
                // package's computed content hash (§2.5). Recursion
                // grounds out because every package's own imports are
                // verified the same way.
                let expected = self
                    .ontology_docs
                    .get(target)
                    .map(allod_core::package_hash);
                match expected {
                    Some(Ok(expected)) if expected != hash => self.error(
                        path,
                        &where_,
                        &format!(
                            "import {target:?} state_hash {hash} does not match \
                             the package's content hash {expected} \
                             (run: allod-vectors hashes)"
                        ),
                    ),
                    Some(Err(err)) => self.error(
                        path,
                        &where_,
                        &format!("import {target:?} cannot be hashed: {err}"),
                    ),
                    _ => {}
                }
            }
        }
        let imports: BTreeSet<String> = self
            .registry
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

        if let Some(smap) = doc.get("structs").and_then(Value::as_mapping) {
            for (sname, sdef) in smap {
                let Some(sname) = sname.as_str() else { continue };
                let swhere = format!("structs.{sname}");
                let other_owners: Vec<String> = self
                    .registry
                    .packages
                    .iter()
                    .filter(|(opkg, op)| {
                        *opkg != &pkg && op.structs.get(sname).is_some()
                    })
                    .map(|(opkg, _)| opkg.clone())
                    .collect();
                for owner in other_owners {
                    self.error(
                        path,
                        &swhere,
                        &format!(
                            "struct name also declared by package {owner:?}; \
                             struct names are graph-global (§2.1)"
                        ),
                    );
                }
                let Some(fields) = sdef.as_mapping() else {
                    self.error(path, &swhere, "struct must be a map of fields");
                    continue;
                };
                for (fname, fdef) in fields {
                    let Some(fname) = fname.as_str() else { continue };
                    let fwhere = format!("{swhere}.{fname}");
                    match fdef.get("type") {
                        Some(ftype) => self.check_type_expr(path, &fwhere, ftype),
                        None => self.error(path, &fwhere, "field needs a type"),
                    }
                    if let Some(req) = fdef.get("required") {
                        if !req.is_bool() {
                            self.error(path, &fwhere, "required must be a bool");
                        }
                    }
                }
            }
        }

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
                                self.registry.resolve_type(t, Some(&pkg)).is_none()
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
                    match self.registry.resolve_type(parent, Some(&pkg)) {
                        None => self.error(
                            path,
                            &twhere,
                            &format!("extends {parent:?} does not resolve"),
                        ),
                        Some(_) if !foreign_ok(parent) => {
                            self.error(
                                path,
                                &twhere,
                                &format!(
                                    "extends {parent:?} but package is not imported"
                                ),
                            );
                        }
                        Some((ppkg, pname, _)) => {
                            let inherited = self.registry.collected_attrs(&ppkg, &pname);
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
                        if self.registry.resolve_type(reference, Some(&pkg)).is_none()
                        {
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
                self.check_validation_rule(path, &pkg, rule);
            }
        }
    }

    // ---------------- validation-rule checks (§2.1) ----------------

    fn check_validation_rule(&mut self, path: &Path, pkg: &str, rule: &Value) {
        let rname = get_str(rule, "name").unwrap_or("<unnamed>").to_string();
        let rwhere = format!("validation_rules.{rname}");
        if get_str(rule, "name").is_none() {
            self.error(path, &rwhere, "rules need a name (§2.1)");
        }
        let (Some(on), Some(require)) = (rule.get("on"), rule.get("require")) else {
            self.error(path, &rwhere, "rules need on and require (§2.1)");
            return;
        };
        if let Some(map) = on.as_mapping() {
            for key in map.keys().filter_map(Value::as_str) {
                if !matches!(key, "type" | "operation" | "where") {
                    self.error(
                        path,
                        &rwhere,
                        &format!("unknown on key {key:?} (§2.1)"),
                    );
                }
            }
        }
        let mut on_attrs = None;
        match get_str(on, "type") {
            None => self.error(path, &rwhere, "on needs a type (§2.1)"),
            Some(t) => match self.registry.resolve_type(t, Some(pkg)) {
                None => self.error(
                    path,
                    &rwhere,
                    &format!("on type {t:?} does not resolve"),
                ),
                Some((tpkg, tname, _)) => {
                    on_attrs = Some(self.registry.collected_attrs(&tpkg, &tname));
                }
            },
        }
        if let Some(ops) = on.get("operation") {
            let ops: Vec<&str> = match ops {
                Value::Sequence(seq) => seq.iter().filter_map(Value::as_str).collect(),
                other => other.as_str().into_iter().collect(),
            };
            for op in ops {
                if !matches!(op, "create" | "update") {
                    self.error(
                        path,
                        &rwhere,
                        &format!("on.operation {op:?} must be create or update (§2.1)"),
                    );
                }
            }
        }
        if let Some(cond) = on.get("where") {
            self.check_condition(
                path,
                &format!("{rwhere}.on.where"),
                pkg,
                on_attrs.as_ref(),
                cond,
            );
        }
        self.check_condition(
            path,
            &format!("{rwhere}.require"),
            pkg,
            on_attrs.as_ref(),
            require,
        );
    }

    fn check_condition(
        &mut self,
        path: &Path,
        where_: &str,
        pkg: &str,
        attrs: Option<&std::collections::BTreeMap<String, Value>>,
        cond: &Value,
    ) {
        let Some(map) = cond.as_mapping() else {
            self.error(
                path,
                where_,
                &format!("condition must be a map, got {cond:?}"),
            );
            return;
        };
        for combinator in ["all", "any"] {
            if let Some(subs) = cond.get(combinator) {
                if map.len() > 1 {
                    self.error(
                        path,
                        where_,
                        &format!("{combinator} does not mix with other keys"),
                    );
                }
                match subs.as_sequence() {
                    Some(seq) => {
                        for (i, sub) in seq.iter().enumerate() {
                            self.check_condition(
                                path,
                                &format!("{where_}.{combinator}[{i}]"),
                                pkg,
                                attrs,
                                sub,
                            );
                        }
                    }
                    None => self.error(
                        path,
                        where_,
                        &format!("{combinator} takes a list of conditions"),
                    ),
                }
                return;
            }
        }
        if let Some(sub) = cond.get("not") {
            if map.len() > 1 {
                self.error(path, where_, "not does not mix with other keys");
            }
            self.check_condition(path, &format!("{where_}.not"), pkg, attrs, sub);
            return;
        }
        if let Some(edge) = cond.get("edge") {
            if map.len() > 1 {
                self.error(path, where_, "edge does not mix with other keys");
            }
            self.check_edge_cond(path, where_, pkg, edge);
            return;
        }
        if cond.get("attr").is_some() {
            self.check_attr_cond(path, where_, cond);
            if let (Some(attrs), Some(name)) = (attrs, get_str(cond, "attr")) {
                if !attrs.contains_key(name) {
                    self.error(
                        path,
                        where_,
                        &format!("attr {name:?} is not declared on the rule's type"),
                    );
                }
            }
            return;
        }
        self.error(
            path,
            where_,
            "condition needs one of attr, edge, all, any, not (§2.1)",
        );
    }

    fn check_attr_cond(&mut self, path: &Path, where_: &str, cond: &Value) {
        if let Some(map) = cond.as_mapping() {
            for key in map.keys().filter_map(Value::as_str) {
                if !matches!(key, "attr" | "equals" | "in" | "present") {
                    self.error(
                        path,
                        where_,
                        &format!("unknown attribute-condition key {key:?} (§2.1)"),
                    );
                }
            }
        }
        if get_str(cond, "attr").is_none() {
            self.error(path, where_, "attribute condition needs attr (§2.1)");
        }
        if let Some(present) = cond.get("present") {
            if !present.is_bool() {
                self.error(path, where_, "present must be a bool");
            }
        }
        if let Some(values) = cond.get("in") {
            if !values.is_sequence() {
                self.error(path, where_, "in takes a list of values");
            }
        }
    }

    fn check_edge_cond(&mut self, path: &Path, where_: &str, pkg: &str, edge: &Value) {
        let Some(map) = edge.as_mapping() else {
            self.error(path, where_, "edge condition must be a map (§2.1)");
            return;
        };
        for key in map.keys().filter_map(Value::as_str) {
            if !matches!(key, "type" | "direction" | "min" | "within" | "target_where")
            {
                self.error(
                    path,
                    where_,
                    &format!("unknown edge-condition key {key:?} (§2.1)"),
                );
            }
        }
        match get_str(edge, "type") {
            None => self.error(path, where_, "edge condition needs a type"),
            Some(t) => {
                let resolved = if bare(t).contains('/') {
                    self.registry.resolve_edge_type(t).is_some()
                } else {
                    self.registry
                        .packages
                        .get(pkg)
                        .is_some_and(|p| p.edges.get(bare(t)).is_some())
                };
                if !resolved {
                    self.error(
                        path,
                        where_,
                        &format!("edge type {t:?} does not resolve"),
                    );
                }
            }
        }
        match get_str(edge, "direction") {
            Some("in" | "out") => {}
            other => self.error(
                path,
                where_,
                &format!("direction {other:?} must be in or out (§2.1)"),
            ),
        }
        if let Some(min) = edge.get("min") {
            if min.as_i64().is_none_or(|m| m < 1) {
                self.error(path, where_, "min must be an integer >= 1");
            }
        }
        if let Some(within) = get_str(edge, "within") {
            if !matches!(within, "changeset" | "state") {
                self.error(
                    path,
                    where_,
                    &format!("within {within:?} must be changeset or state (§2.1)"),
                );
            }
        }
        if let Some(tw) = edge.get("target_where") {
            self.check_condition(path, &format!("{where_}.target_where"), pkg, None, tw);
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
                    if !seen.contains(parent) && !self.registry.term_exists(parent) {
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
            if !self.registry.term_exists(region) {
                self.error(
                    path,
                    where_,
                    &format!("region {region:?} resolves to no taxonomy term"),
                );
            }
        }
        if let Some(tref) = get_str(sel, "type") {
            if self.registry.resolve_type(tref, None).is_none() {
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
        if let Some(imported) = sel.get("imported") {
            let ok = matches!(imported, Value::Bool(true))
                || imported.as_str().is_some_and(has_algo_prefix);
            if !ok {
                self.error(
                    path,
                    where_,
                    "imported must be true or a graph ID hash (§4.1)",
                );
            }
        }
        if let Some(cond) = sel.get("where") {
            self.check_attr_cond(path, &format!("{where_}.where"), cond);
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
        let Some((pkg, tname, _)) = self.registry.resolve_type(reference, None) else {
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
        let attrs = self.registry.collected_attrs(&pkg, &tname);
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
        if let Some(given_map) = doc.get("attributes").and_then(Value::as_mapping) {
            for (aname, aval) in given_map {
                let Some(aname) = aname.as_str() else { continue };
                let Some(texpr) = attrs
                    .get(aname)
                    .and_then(|adef| adef.get("type"))
                    .and_then(Value::as_str)
                    .map(String::from)
                else {
                    continue;
                };
                self.check_value(path, &format!("{where_}.{aname}"), &texpr, aval);
            }
        }
    }

    /// Check an instance value against the declared type where the
    /// type constrains values: enum membership, selector grammar, and
    /// struct shape. Other base types are not value-checked here.
    fn check_value(&mut self, path: &Path, where_: &str, texpr: &str, val: &Value) {
        let texpr = texpr.trim();
        if let Some(inner) = texpr
            .strip_prefix("list<")
            .and_then(|rest| rest.strip_suffix('>'))
        {
            if let Some(seq) = val.as_sequence() {
                for (i, item) in seq.iter().enumerate() {
                    self.check_value(path, &format!("{where_}[{i}]"), inner, item);
                }
            }
            return;
        }
        if let Some(inner) = texpr
            .strip_prefix("enum<")
            .and_then(|rest| rest.strip_suffix('>'))
        {
            let symbols: Vec<&str> = inner.split('|').map(str::trim).collect();
            match val.as_str() {
                Some(s) if symbols.contains(&s) => {}
                Some(s) => self.error(
                    path,
                    where_,
                    &format!("value {s:?} not in enum<{inner}> (§1.3)"),
                ),
                None => self.error(path, where_, "enum value must be a string"),
            }
            return;
        }
        if texpr == "selector" {
            self.check_selector(path, where_, val);
            return;
        }
        if let Some((_, sdef)) = self.registry.resolve_struct(texpr) {
            self.check_struct_value(path, where_, &sdef, val);
        }
    }

    fn check_struct_value(
        &mut self,
        path: &Path,
        where_: &str,
        sdef: &Value,
        val: &Value,
    ) {
        let Some(vmap) = val.as_mapping() else {
            self.error(path, where_, "struct value must be a map (§2.1)");
            return;
        };
        let Some(fields) = sdef.as_mapping() else { return };
        for (fname, fdef) in fields {
            let Some(fname) = fname.as_str() else { continue };
            let required = fdef.get("required").and_then(Value::as_bool) == Some(true);
            if required && val.get(fname).is_none() {
                self.error(
                    path,
                    where_,
                    &format!("missing required struct field {fname:?}"),
                );
            }
            if let (Some(fval), Some(ftype)) = (
                val.get(fname),
                fdef.get("type").and_then(Value::as_str).map(String::from),
            ) {
                self.check_value(path, &format!("{where_}.{fname}"), &ftype, fval);
            }
        }
        for key in vmap.keys().filter_map(Value::as_str) {
            if sdef.get(key).is_none() {
                self.error(
                    path,
                    where_,
                    &format!("struct field {key:?} is not declared"),
                );
            }
        }
    }

    fn check_edge_payload(&mut self, path: &Path, where_: &str, doc: &Value) {
        let reference = get_str(doc, "type").unwrap_or("<missing>");
        let Some((_, edef)) = self.registry.resolve_edge_type(reference) else {
            self.error(
                path,
                where_,
                &format!("edge type {reference:?} does not resolve"),
            );
            return;
        };
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
            for (aname, aval) in given {
                let Some(aname) = aname.as_str() else { continue };
                if !declared.contains(aname) {
                    self.error(
                        path,
                        where_,
                        &format!(
                            "attribute {aname:?} is not declared on edge type \
                             {reference}"
                        ),
                    );
                    continue;
                }
                let Some(texpr) = edef
                    .get("attributes")
                    .and_then(|attrs| attrs.get(aname))
                    .and_then(|adef| adef.get("type"))
                    .and_then(Value::as_str)
                    .map(String::from)
                else {
                    continue;
                };
                self.check_value(path, &format!("{where_}.{aname}"), &texpr, aval);
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
            if !self.registry.term_exists(term) {
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
        let loaded = allod_core::load_dir(root);
        self.registry = loaded.registry;
        self.structs = self.registry.struct_names();
        for (_, doc) in &loaded.docs {
            if let Some(name) = get_str(doc, "ontology") {
                self.ontology_docs.insert(name.to_string(), doc.clone());
            }
        }
        for issue in &loaded.issues {
            self.errors.push(format!(
                "{}: {}: {}",
                issue.path.display(),
                issue.context,
                issue.message
            ));
        }
        for (path, doc) in &loaded.docs {
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
            self.registry.packages.len(),
            self.registry.taxonomies.len(),
            self.errors.len(),
            self.warnings.len()
        );
        u8::from(!self.errors.is_empty())
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
