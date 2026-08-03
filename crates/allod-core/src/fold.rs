//! The fold (§3.2.4): materialize state by applying changesets in
//! order, rejecting schema violations, dangling references, and
//! prior-revision mismatches, and evaluating the structured
//! validation rules of §2.1.
//!
//! Scope: linear history (single parent). Merges with `resolve`
//! operations and redaction replay are the next milestone; a
//! changeset that needs them is rejected with a clear message rather
//! than misapplied.

use crate::model::{changeset_hash, revision_hash, state_entry, state_root};
use crate::registry::Registry;
use crate::{bare, get_str};
use serde_yaml::Value;
use std::collections::BTreeMap;

#[derive(Clone)]
pub struct Obj {
    /// The payload as created or updated: kind, id, type, attributes,
    /// from/to, and so on, without `rev`.
    pub content: Value,
    pub rev: String,
    pub deleted: bool,
}

#[derive(Clone, Default)]
pub struct State {
    /// Keyed by (kind, logical id) — the state-tree order of §1.7.
    pub objects: BTreeMap<(String, String), Obj>,
}

impl State {
    pub fn get_live(&self, kind: &str, id: &str) -> Option<&Obj> {
        self.objects
            .get(&(kind.to_string(), id.to_string()))
            .filter(|o| !o.deleted)
    }

    /// Resolve a reference like `node:<id>` to a live object.
    pub fn resolve_ref<'s, 'r>(&'s self, reference: &'r str) -> Option<(&'r str, &'s Obj)> {
        let (kind, id) = reference.split_once(':')?;
        self.get_live(kind, id).map(|o| (kind, o))
    }

    /// Live classification terms whose subject is `reference`.
    pub fn classifications_of(&self, reference: &str) -> Vec<String> {
        self.objects
            .iter()
            .filter(|((kind, _), obj)| {
                kind == "classification"
                    && !obj.deleted
                    && get_str(&obj.content, "subject") == Some(reference)
            })
            .filter_map(|(_, obj)| get_str(&obj.content, "term").map(String::from))
            .collect()
    }

    /// Find a principal object by `principal:<name>` reference:
    /// a core/User, core/Agent, or core/Service node whose
    /// display_name matches.
    pub fn find_principal(&self, reference: &str) -> Option<(&str, &Obj)> {
        let name = reference.strip_prefix("principal:")?;
        for ((kind, _), obj) in &self.objects {
            if kind != "node" || obj.deleted {
                continue;
            }
            let tref = get_str(&obj.content, "type").unwrap_or("");
            let bare_type = bare(tref);
            let principal_kind = match bare_type {
                "core/User" => "user",
                "core/Agent" => "agent",
                "core/Service" => "service",
                _ => continue,
            };
            let display = obj
                .content
                .get("attributes")
                .and_then(|a| get_str(a, "display_name"));
            if display == Some(name) {
                return Some((principal_kind, obj));
            }
        }
        None
    }

    /// A principal's hex public key for a key ID, from its KeyRecord
    /// list (§6.2).
    pub fn public_key_of(&self, principal_ref: &str, key_id: &str) -> Option<String> {
        let (_, obj) = self.find_principal(principal_ref)?;
        let keys = obj.content.get("attributes")?.get("keys")?.as_sequence()?;
        for record in keys {
            if get_str(record, "key_id") == Some(key_id)
                && get_str(record, "status") == Some("active")
            {
                return get_str(record, "public").map(String::from);
            }
        }
        None
    }

    pub fn entries(&self) -> Vec<Value> {
        self.objects
            .iter()
            .map(|((kind, id), obj)| state_entry(kind, id, &obj.rev, obj.deleted))
            .collect()
    }

    pub fn state_hash(&self) -> Result<String, String> {
        state_root(&self.entries())
    }

    /// Apply one changeset. Validation failures reject the whole
    /// changeset and leave the state untouched (§3.2.1 atomicity).
    pub fn apply_changeset(&mut self, reg: &Registry, cs: &Value) -> Result<(), String> {
        let (computed, _, _, _) = changeset_hash(cs)?;
        if let Some(stored) = get_str(cs, "hash") {
            if stored != computed {
                return Err(format!(
                    "changeset hash {stored} does not recompute (got {computed})"
                ));
            }
        }
        let parents = cs
            .get("parents")
            .and_then(Value::as_sequence)
            .ok_or("changeset needs parents")?;
        if parents.len() > 1 {
            return Err("merge changesets are not yet supported by this fold".into());
        }
        let ops = cs
            .get("operations")
            .and_then(Value::as_sequence)
            .ok_or("changeset needs an operations list")?;

        let mut working = self.clone();
        let mut created_edges: Vec<Value> = Vec::new();
        let mut touched: Vec<(String, Value)> = Vec::new(); // (verb, payload)

        for op in ops {
            let map = op.as_mapping().ok_or("operation must be a map")?;
            let (verb, payload) = map.iter().next().ok_or("empty operation")?;
            let verb = verb.as_str().ok_or("operation verb must be a string")?;
            let kind = get_str(payload, "kind")
                .ok_or("operation payload needs kind")?
                .to_string();
            let id = get_str(payload, "id")
                .ok_or("operation payload needs id")?
                .to_string();
            let key = (kind.clone(), id.clone());
            match verb {
                "create" => {
                    if working.objects.contains_key(&key) {
                        return Err(format!("create of existing object {key:?}"));
                    }
                    validate_payload(reg, &working, &kind, payload)?;
                    let rev = revision_hash(payload)?;
                    if kind == "edge" {
                        created_edges.push(payload.clone());
                    }
                    working.objects.insert(
                        key,
                        Obj {
                            content: payload.clone(),
                            rev,
                            deleted: false,
                        },
                    );
                }
                "update" => {
                    let prior = get_str(payload, "prior").ok_or("update needs prior")?;
                    let current = working
                        .objects
                        .get(&key)
                        .ok_or_else(|| format!("update of unknown object {key:?}"))?;
                    if current.deleted || current.rev != prior {
                        return Err(format!("prior-revision mismatch on {key:?}"));
                    }
                    let mut content = payload.clone();
                    if let Some(m) = content.as_mapping_mut() {
                        m.remove("prior");
                    }
                    validate_payload(reg, &working, &kind, &content)?;
                    let rev = revision_hash(&content)?;
                    working.objects.insert(
                        key,
                        Obj {
                            content,
                            rev,
                            deleted: false,
                        },
                    );
                }
                "delete" => {
                    let prior = get_str(payload, "prior").ok_or("delete needs prior")?;
                    let current = working
                        .objects
                        .get(&key)
                        .ok_or_else(|| format!("delete of unknown object {key:?}"))?;
                    if current.deleted || current.rev != prior {
                        return Err(format!("prior-revision mismatch on {key:?}"));
                    }
                    let obj = Obj {
                        content: current.content.clone(),
                        rev: current.rev.clone(),
                        deleted: true,
                    };
                    working.objects.insert(key, obj);
                }
                other => {
                    return Err(format!(
                        "operation {other:?} is not yet supported by this fold"
                    ))
                }
            }
            touched.push((verb.to_string(), payload.clone()));
        }

        // Dangling references (§3.2.4): every live edge's endpoints
        // must be live.
        for ((kind, id), obj) in &working.objects {
            if kind != "edge" || obj.deleted {
                continue;
            }
            for side in ["from", "to"] {
                let reference = get_str(&obj.content, side).unwrap_or("");
                if working.resolve_ref(reference).is_none() {
                    return Err(format!(
                        "edge {id} has a dangling {side} reference {reference:?}"
                    ));
                }
            }
        }

        // Validation rules (§2.1).
        for (pkg, package) in &reg.packages {
            let Some(rules) = package.rules.as_sequence() else { continue };
            for rule in rules {
                check_rule(reg, &working, &created_edges, &touched, pkg, rule)?;
            }
        }

        *self = working;
        Ok(())
    }
}

// ---------------- payload validation ----------------

fn validate_payload(
    reg: &Registry,
    state: &State,
    kind: &str,
    payload: &Value,
) -> Result<(), String> {
    match kind {
        "node" => {
            let tref = get_str(payload, "type").ok_or("node needs a type")?;
            let (tpkg, tname, _) = reg
                .resolve_type(tref, None)
                .ok_or_else(|| format!("node type {tref:?} does not resolve"))?;
            let attrs = reg.collected_attrs(&tpkg, &tname);
            let given = payload.get("attributes");
            for (aname, adef) in &attrs {
                let required =
                    adef.get("required").and_then(Value::as_bool) == Some(true);
                let present = given.is_some_and(|g| g.get(aname.as_str()).is_some());
                if required && !present {
                    return Err(format!(
                        "missing required attribute {aname:?} of {tpkg}/{tname}"
                    ));
                }
            }
            if let Some(given) = given.and_then(Value::as_mapping) {
                for (aname, aval) in given {
                    let Some(aname) = aname.as_str() else { continue };
                    let Some(adef) = attrs.get(aname) else {
                        return Err(format!(
                            "attribute {aname:?} is not declared on {tpkg}/{tname}"
                        ));
                    };
                    if let Some(texpr) = adef.get("type").and_then(Value::as_str) {
                        validate_value(reg, texpr, aval)
                            .map_err(|e| format!("attribute {aname:?}: {e}"))?;
                    }
                }
            }
            Ok(())
        }
        "edge" => {
            let tref = get_str(payload, "type").ok_or("edge needs a type")?;
            let (_, edef) = reg
                .resolve_edge_type(tref)
                .ok_or_else(|| format!("edge type {tref:?} does not resolve"))?;
            for (side, allowed_key) in [("from", "domain"), ("to", "range")] {
                let reference = get_str(payload, side)
                    .ok_or_else(|| format!("edge needs {side}"))?;
                let (rkind, target) = state
                    .resolve_ref(reference)
                    .ok_or_else(|| format!("edge {side} {reference:?} does not resolve"))?;
                if rkind != "node" {
                    return Err(format!("edge {side} must reference a node"));
                }
                let target_type = get_str(&target.content, "type").unwrap_or("");
                let allowed = edef.get(allowed_key).ok_or("edge type missing sides")?;
                let candidates: Vec<&str> = match allowed {
                    Value::Sequence(seq) => seq.iter().filter_map(Value::as_str).collect(),
                    other => other.as_str().into_iter().collect(),
                };
                let pkg = bare(tref).rsplit_once('/').map(|(p, _)| p);
                let ok = candidates
                    .iter()
                    .any(|a| reg.type_satisfies(target_type, a, pkg));
                if !ok {
                    return Err(format!(
                        "edge {side} type {target_type:?} violates {allowed_key} of {tref}"
                    ));
                }
            }
            Ok(())
        }
        "classification" => {
            let subject = get_str(payload, "subject").ok_or("classification needs subject")?;
            if state.resolve_ref(subject).is_none() {
                return Err(format!("classification subject {subject:?} does not resolve"));
            }
            let term = get_str(payload, "term").ok_or("classification needs term")?;
            if !reg.term_exists(term) {
                return Err(format!("term {term:?} resolves to no taxonomy term"));
            }
            Ok(())
        }
        "document" => {
            get_str(payload, "content_hash")
                .ok_or("document needs content_hash")
                .map(|_| ())
                .map_err(String::from)
        }
        other => Err(format!("unknown object kind {other:?}")),
    }
}

fn validate_value(reg: &Registry, texpr: &str, val: &Value) -> Result<(), String> {
    let texpr = texpr.trim();
    if let Some(inner) = texpr.strip_prefix("list<").and_then(|r| r.strip_suffix('>')) {
        if let Some(seq) = val.as_sequence() {
            for item in seq {
                validate_value(reg, inner, item)?;
            }
        }
        return Ok(());
    }
    if let Some(inner) = texpr.strip_prefix("enum<").and_then(|r| r.strip_suffix('>')) {
        let symbols: Vec<&str> = inner.split('|').map(str::trim).collect();
        return match val.as_str() {
            Some(s) if symbols.contains(&s) => Ok(()),
            Some(s) => Err(format!("value {s:?} not in enum<{inner}>")),
            None => Err("enum value must be a string".into()),
        };
    }
    if let Some((_, sdef)) = reg.resolve_struct(texpr) {
        let Some(fields) = sdef.as_mapping() else { return Ok(()) };
        for (fname, fdef) in fields {
            let Some(fname) = fname.as_str() else { continue };
            let required = fdef.get("required").and_then(Value::as_bool) == Some(true);
            if required && val.get(fname).is_none() {
                return Err(format!("missing required struct field {fname:?}"));
            }
            if let (Some(fval), Some(ftype)) =
                (val.get(fname), fdef.get("type").and_then(Value::as_str))
            {
                validate_value(reg, ftype, fval)?;
            }
        }
    }
    Ok(())
}

// ---------------- validation rules (§2.1) ----------------

fn check_rule(
    reg: &Registry,
    state: &State,
    created_edges: &[Value],
    touched: &[(String, Value)],
    pkg: &str,
    rule: &Value,
) -> Result<(), String> {
    let rname = get_str(rule, "name").unwrap_or("<unnamed>");
    let Some(on) = rule.get("on") else { return Ok(()) };
    let Some(on_type) = get_str(on, "type") else { return Ok(()) };
    let on_ops: Vec<&str> = match on.get("operation") {
        Some(Value::Sequence(seq)) => seq.iter().filter_map(Value::as_str).collect(),
        Some(other) => other.as_str().into_iter().collect(),
        None => vec!["create", "update"],
    };
    for (verb, payload) in touched {
        if get_str(payload, "kind") != Some("node") || !on_ops.contains(&verb.as_str()) {
            continue;
        }
        let tref = get_str(payload, "type").unwrap_or("");
        if !reg.type_satisfies(tref, on_type, Some(pkg)) {
            continue;
        }
        let id = get_str(payload, "id").unwrap_or("");
        let obj_ref = format!("node:{id}");
        // The rule's conditions see the object as it exists after the
        // changeset applies.
        let Some((_, obj)) = state.resolve_ref(&obj_ref) else { continue };
        let content = obj.content.clone();
        if let Some(cond) = on.get("where") {
            if !eval_condition(reg, state, created_edges, &obj_ref, &content, pkg, cond)? {
                continue;
            }
        }
        let require = rule.get("require").ok_or("rule needs require")?;
        if !eval_condition(reg, state, created_edges, &obj_ref, &content, pkg, require)? {
            return Err(format!(
                "validation rule {rname:?} fails for {obj_ref} ({pkg})"
            ));
        }
    }
    Ok(())
}

fn eval_condition(
    reg: &Registry,
    state: &State,
    created_edges: &[Value],
    obj_ref: &str,
    obj: &Value,
    pkg: &str,
    cond: &Value,
) -> Result<bool, String> {
    if let Some(subs) = cond.get("all").and_then(Value::as_sequence) {
        for sub in subs {
            if !eval_condition(reg, state, created_edges, obj_ref, obj, pkg, sub)? {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    if let Some(subs) = cond.get("any").and_then(Value::as_sequence) {
        for sub in subs {
            if eval_condition(reg, state, created_edges, obj_ref, obj, pkg, sub)? {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    if let Some(sub) = cond.get("not") {
        return Ok(!eval_condition(reg, state, created_edges, obj_ref, obj, pkg, sub)?);
    }
    if let Some(edge) = cond.get("edge") {
        return eval_edge_condition(reg, state, created_edges, obj_ref, pkg, edge);
    }
    if let Some(attr) = get_str(cond, "attr") {
        let value = obj.get("attributes").and_then(|a| a.get(attr));
        if let Some(present) = cond.get("present").and_then(Value::as_bool) {
            return Ok(value.is_some() == present);
        }
        if let Some(expected) = cond.get("equals") {
            return Ok(value == Some(expected));
        }
        if let Some(options) = cond.get("in").and_then(Value::as_sequence) {
            return Ok(value.is_some_and(|v| options.contains(v)));
        }
        return Ok(value.is_some());
    }
    Err(format!("unrecognized condition {cond:?}"))
}

fn eval_edge_condition(
    reg: &Registry,
    state: &State,
    created_edges: &[Value],
    obj_ref: &str,
    pkg: &str,
    edge: &Value,
) -> Result<bool, String> {
    let etype = get_str(edge, "type").ok_or("edge condition needs type")?;
    let direction = get_str(edge, "direction").ok_or("edge condition needs direction")?;
    let min = edge.get("min").and_then(Value::as_u64).unwrap_or(1);
    let within = get_str(edge, "within").unwrap_or("state");
    let near_side = if direction == "out" { "from" } else { "to" };
    let far_side = if direction == "out" { "to" } else { "from" };

    let type_matches = |candidate: &str| -> bool {
        let cbare = bare(candidate);
        let ebare = bare(etype);
        cbare == ebare
            || cbare == format!("{pkg}/{ebare}")
            || cbare.rsplit_once('/').map(|(_, n)| n) == Some(ebare)
    };

    let mut count = 0u64;
    let candidates: Vec<Value> = if within == "changeset" {
        created_edges.to_vec()
    } else {
        state
            .objects
            .iter()
            .filter(|((kind, _), o)| kind == "edge" && !o.deleted)
            .map(|(_, o)| o.content.clone())
            .collect()
    };
    for candidate in candidates {
        let ctype = get_str(&candidate, "type").unwrap_or("");
        if !type_matches(ctype) || get_str(&candidate, near_side) != Some(obj_ref) {
            continue;
        }
        if let Some(tw) = edge.get("target_where") {
            let far_ref = get_str(&candidate, far_side).unwrap_or("");
            let Some((_, far)) = state.resolve_ref(far_ref) else { continue };
            let far_content = far.content.clone();
            if !eval_condition(reg, state, created_edges, far_ref, &far_content, pkg, tw)? {
                continue;
            }
        }
        count += 1;
    }
    Ok(count >= min)
}
