//! Governance (Part 4): evaluate a proposal against a policy to
//! produce a requirement checklist, and check whether decision
//! records and attestation envelopes satisfy it.
//!
//! Evaluation implements the §4.1 semantics exactly: bare multi-key
//! selectors conjoin, `all`/`any`/`not` compose, matched requirements
//! union, a carve-out exempts a change from its own rule only, and an
//! operation matched by no rule falls to the default posture. Region
//! selectors see classifications as of the parent state plus those
//! the proposal itself creates, expanded through the taxonomy
//! ancestor union (§2.2).

use crate::fold::State;
use crate::hash::sha256_hex;
use crate::model::changeset_hash;
use crate::registry::Registry;
use crate::{bare, canonical_cbor, get_str, sign};
use serde_yaml::{Mapping, Value};
use std::collections::BTreeSet;

#[derive(Default, Debug)]
pub struct Checklist {
    pub matched_rules: BTreeSet<String>,
    /// (role, quorum) — union of all matched reviewer requirements.
    pub reviewers: Vec<(String, u64)>,
    /// Required attester classes.
    pub attestations: Vec<String>,
    /// Some operation matched no rule under a restricted posture, so
    /// the author must hold root authority (§4.1).
    pub root_required: bool,
}

impl Checklist {
    pub fn is_trivial(&self) -> bool {
        self.reviewers.is_empty() && self.attestations.is_empty() && !self.root_required
    }
}

/// The policy version identifier decision records bind to (§4.3):
/// the policy document's content hash, same bridge as package hashes.
pub fn policy_context(policy: &Value) -> Result<String, String> {
    Ok(sha256_hex("package", &canonical_cbor(policy)?))
}

/// Per-operation context the selectors key on.
struct OpContext {
    verb: String,
    kind: String,
    type_ref: String,
    basis: Option<String>,
    imported: bool,
    /// Ancestor-closed region set (§4.1 timing rule).
    regions: BTreeSet<String>,
    payload: Value,
    /// Sugar-verb labels derived from the meta type of this op (§3.2.2).
    ///
    /// `compile_schema_ops` always emits `create`/`update` verbs; policy
    /// rules written with `operation: define-type` (etc.) match via this
    /// mapping so that ontologies/*/policy-*.yaml files need no changes.
    ///
    /// Mapping (applied at selector-match time, not at op-emit time):
    /// - `meta/EntityType`, `meta/EdgeType`, `meta/Struct` → `"define-type"`
    /// - `meta/Policy`                                     → `"set-policy"`
    /// - `meta/TaxonomyTerm` with `status == "deprecated"` → `"deprecate-term"`
    sugar_verbs: Vec<&'static str>,
}

/// Compute the sugar-verb labels that apply to an op targeting a meta-typed node.
///
/// These labels allow policy rules with `operation: define-type` (etc.) to match
/// `create`/`update` ops on meta nodes without changing the ontologies/ YAML files.
fn meta_sugar_verbs(bare_type: &str, payload: &Value) -> Vec<&'static str> {
    match bare_type {
        "meta/EntityType" | "meta/EdgeType" | "meta/Struct" => vec!["define-type"],
        "meta/Policy" => vec!["set-policy"],
        "meta/TaxonomyTerm" => {
            // Only match deprecate-term when the status attribute is "deprecated".
            let status = payload
                .get("attributes")
                .and_then(|a| get_str(a, "status"));
            if status == Some("deprecated") {
                vec!["deprecate-term"]
            } else {
                vec![]
            }
        }
        _ => vec![],
    }
}

fn op_contexts(reg: &Registry, parent: &State, cs: &Value) -> Result<Vec<OpContext>, String> {
    let ops = cs
        .get("operations")
        .and_then(Value::as_sequence)
        .ok_or("changeset needs operations")?;
    // Classifications created by this proposal, by subject (§4.1).
    let mut created_classifications: Vec<(String, String)> = Vec::new();
    for op in ops {
        let Some((verb, payload)) = op.as_mapping().and_then(|m| m.iter().next()) else {
            continue;
        };
        if verb.as_str() == Some("create")
            && get_str(payload, "kind") == Some("classification")
        {
            if let (Some(subject), Some(term)) =
                (get_str(payload, "subject"), get_str(payload, "term"))
            {
                created_classifications.push((subject.to_string(), term.to_string()));
            }
        }
    }
    let mut contexts = Vec::new();
    for op in ops {
        let map = op.as_mapping().ok_or("operation must be a map")?;
        let (verb, payload) = map.iter().next().ok_or("empty operation")?;
        let verb = verb.as_str().ok_or("operation verb must be a string")?.to_string();
        let kind = get_str(payload, "kind").unwrap_or("").to_string();
        let id = get_str(payload, "id").unwrap_or("").to_string();
        let type_ref = get_str(payload, "type").unwrap_or("").to_string();
        // For delete ops on nodes: resolve the type from parent state so that
        // type: and operation: selectors match deletes of meta nodes (§3.2.2).
        let type_ref = if verb == "delete" && kind == "node" && type_ref.is_empty() {
            parent
                .get_live("node", &id)
                .and_then(|obj| get_str(&obj.content, "type").map(String::from))
                .unwrap_or(type_ref)
        } else {
            type_ref
        };
        let basis = get_str(payload, "basis")
            .or_else(|| {
                payload
                    .get("provenance")
                    .and_then(|p| get_str(p, "method"))
            })
            .map(String::from);
        let imported = payload
            .get("provenance")
            .and_then(|p| p.get("derived_from"))
            .and_then(Value::as_sequence)
            .is_some_and(|seq| {
                seq.iter()
                    .filter_map(Value::as_str)
                    .any(|r| r.starts_with("allod:"))
            });
        let mut terms: Vec<String> = Vec::new();
        if kind == "classification" {
            if let Some(term) = get_str(payload, "term") {
                terms.push(term.to_string());
            }
        } else {
            let obj_ref = format!("{kind}:{id}");
            terms.extend(parent.classifications_of(&obj_ref));
            terms.extend(
                created_classifications
                    .iter()
                    .filter(|(s, _)| *s == obj_ref)
                    .map(|(_, t)| t.clone()),
            );
        }
        let mut regions = BTreeSet::new();
        for term in terms {
            regions.extend(reg.term_closure(&term));
        }
        // Compute sugar-verb labels for meta-typed node ops.
        let sugar_verbs = if (kind == "node") && !type_ref.is_empty() {
            use crate::meta::is_meta_type;
            if is_meta_type(&type_ref) {
                meta_sugar_verbs(crate::bare(&type_ref), payload)
            } else {
                vec![]
            }
        } else {
            vec![]
        };

        contexts.push(OpContext {
            verb,
            kind,
            type_ref,
            basis,
            imported,
            regions,
            payload: payload.clone(),
            sugar_verbs,
        });
    }
    Ok(contexts)
}

fn selector_matches(reg: &Registry, sel: &Value, author_kind: &str, ctx: &OpContext) -> bool {
    if let Some(subs) = sel.get("all").and_then(Value::as_sequence) {
        return subs.iter().all(|s| selector_matches(reg, s, author_kind, ctx));
    }
    if let Some(subs) = sel.get("any").and_then(Value::as_sequence) {
        return subs.iter().any(|s| selector_matches(reg, s, author_kind, ctx));
    }
    if let Some(sub) = sel.get("not") {
        return !selector_matches(reg, sub, author_kind, ctx);
    }
    // A bare map conjoins its keys (§4.1).
    if let Some(kind) = get_str(sel, "author_kind") {
        if kind != author_kind {
            return false;
        }
    }
    if let Some(basis) = get_str(sel, "basis") {
        if ctx.basis.as_deref() != Some(basis) {
            return false;
        }
    }
    if let Some(region) = get_str(sel, "region") {
        if !ctx.regions.contains(bare(region)) {
            return false;
        }
    }
    if let Some(tref) = get_str(sel, "type") {
        if ctx.kind == "node" || ctx.kind == "edge" {
            if !reg.type_satisfies(&ctx.type_ref, tref, None) {
                return false;
            }
        } else {
            return false;
        }
    }
    if let Some(ops) = sel.get("operation") {
        let ops: Vec<&str> = match ops {
            Value::Sequence(seq) => seq.iter().filter_map(Value::as_str).collect(),
            other => other.as_str().into_iter().collect(),
        };
        // A raw verb match OR any sugar-verb label covers this op (§3.2.2).
        let matched = ops.contains(&ctx.verb.as_str())
            || ctx.sugar_verbs.iter().any(|sv| ops.contains(sv));
        if !matched {
            return false;
        }
    }
    if let Some(imported) = sel.get("imported") {
        let wanted = matches!(imported, Value::Bool(true)) || imported.as_str().is_some();
        if wanted && !ctx.imported {
            return false;
        }
    }
    if let Some(cond) = sel.get("where") {
        if let Some(attr) = get_str(cond, "attr") {
            let value = ctx.payload.get("attributes").and_then(|a| a.get(attr));
            if let Some(expected) = cond.get("equals") {
                if value != Some(expected) {
                    return false;
                }
            } else if let Some(options) = cond.get("in").and_then(Value::as_sequence) {
                if !value.is_some_and(|v| options.contains(v)) {
                    return false;
                }
            } else if value.is_none() {
                return false;
            }
        }
    }
    if get_str(sel, "substrate").is_some() {
        // Git-substrate selectors never match native operations.
        return false;
    }
    true
}

/// Evaluate a proposal against the policy in force at its parent
/// (§4.3 step 2). `author_kind` is the proposing principal's kind.
pub fn evaluate(
    reg: &Registry,
    policy: &Value,
    parent: &State,
    cs: &Value,
    author_kind: &str,
) -> Result<Checklist, String> {
    let posture = get_str(policy, "default_posture").unwrap_or("restricted");
    let rules = policy
        .get("rules")
        .and_then(Value::as_sequence)
        .ok_or("policy needs rules")?;
    let mut checklist = Checklist::default();
    for ctx in op_contexts(reg, parent, cs)? {
        let mut matched_any = false;
        for rule in rules {
            let (Some(name), Some(select), Some(require)) = (
                get_str(rule, "name"),
                rule.get("select"),
                rule.get("require"),
            ) else {
                continue;
            };
            if !selector_matches(reg, select, author_kind, &ctx) {
                continue;
            }
            matched_any = true;
            checklist.matched_rules.insert(name.to_string());
            if let Some(reviewers) = require.get("reviewers") {
                let entries: Vec<&Value> = match reviewers {
                    Value::Sequence(seq) => seq.iter().collect(),
                    other => vec![other],
                };
                for entry in entries {
                    if let Some(role) = get_str(entry, "role") {
                        let quorum =
                            entry.get("quorum").and_then(Value::as_u64).unwrap_or(1);
                        let req = (role.to_string(), quorum);
                        if !checklist.reviewers.contains(&req) {
                            checklist.reviewers.push(req);
                        }
                    }
                }
            }
            if let Some(att) = require.get("attestation_required") {
                if let Some(class) = get_str(att, "attester_class") {
                    if !checklist.attestations.contains(&class.to_string()) {
                        checklist.attestations.push(class.to_string());
                    }
                }
            }
        }
        if !matched_any && posture == "restricted" {
            checklist.root_required = true;
        }
    }
    Ok(checklist)
}

// ---------------- git-substrate evaluation (§3.3) ----------------

/// Minimal glob: `*` matches any run of characters (including `/`),
/// everything else is literal. Rule patterns in §3.3 policies key on
/// repo, path, and branch shapes; this is the whole grammar.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    fn inner(p: &[u8], t: &[u8]) -> bool {
        match p.first() {
            None => t.is_empty(),
            Some(b'*') => {
                (0..=t.len()).any(|i| inner(&p[1..], &t[i..]))
            }
            Some(c) => t.first() == Some(c) && inner(&p[1..], &t[1..]),
        }
    }
    inner(pattern.as_bytes(), text.as_bytes())
}

/// A git changeset viewed for policy evaluation (§3.3): the repo name,
/// the ref the change targets, and the deterministic operation set as
/// (verb, path) pairs with verb in create|update|delete.
pub struct GitChange {
    pub repo: String,
    pub target_ref: String,
    pub ops: Vec<(String, String)>,
}

/// Evaluate a git change against policy rules whose `select` carries
/// `substrate: git` (§4.1, §3.3). Change-level keys `repo` and
/// `target_ref` glob-match the change; op-level keys `path` and
/// `operation` must co-match at least one op. Requirements union as in
/// `evaluate`. `root_required` is never set here: default-posture
/// fall-through is a native-substrate concept, and the advisory git
/// path reports matched requirements only.
pub fn evaluate_git(policy: &Value, change: &GitChange) -> Result<Checklist, String> {
    let rules = policy
        .get("rules")
        .and_then(Value::as_sequence)
        .ok_or("policy needs rules")?;
    let mut checklist = Checklist::default();
    for rule in rules {
        let (Some(name), Some(select), Some(require)) = (
            get_str(rule, "name"),
            rule.get("select"),
            rule.get("require"),
        ) else {
            continue;
        };
        if get_str(select, "substrate") != Some("git") {
            continue;
        }
        if let Some(repo_pat) = get_str(select, "repo") {
            if !glob_match(repo_pat, &change.repo) {
                continue;
            }
        }
        if let Some(ref_pat) = get_str(select, "target_ref") {
            if !glob_match(ref_pat, &change.target_ref) {
                continue;
            }
        }
        let path_pat = get_str(select, "path");
        let verbs: Option<Vec<&str>> = select.get("operation").map(|ops| match ops {
            Value::Sequence(seq) => seq.iter().filter_map(Value::as_str).collect(),
            other => other.as_str().into_iter().collect(),
        });
        let op_matches = |(verb, path): &(String, String)| -> bool {
            if let Some(pat) = path_pat {
                if !glob_match(pat, path) {
                    return false;
                }
            }
            if let Some(vs) = &verbs {
                if !vs.contains(&verb.as_str()) {
                    return false;
                }
            }
            true
        };
        if !change.ops.iter().any(op_matches) {
            continue;
        }
        checklist.matched_rules.insert(name.to_string());
        if let Some(reviewers) = require.get("reviewers") {
            let entries: Vec<&Value> = match reviewers {
                Value::Sequence(seq) => seq.iter().collect(),
                other => vec![other],
            };
            for entry in entries {
                if let Some(role) = get_str(entry, "role") {
                    let quorum = entry.get("quorum").and_then(Value::as_u64).unwrap_or(1);
                    let req = (role.to_string(), quorum);
                    if !checklist.reviewers.contains(&req) {
                        checklist.reviewers.push(req);
                    }
                }
            }
        }
        if let Some(att) = require.get("attestation_required") {
            if let Some(class) = get_str(att, "attester_class") {
                if !checklist.attestations.contains(&class.to_string()) {
                    checklist.attestations.push(class.to_string());
                }
            }
        }
    }
    Ok(checklist)
}

// ---------------- decision records and envelopes ----------------

/// The payload every decider signs (§4.3): the hash of the record's
/// subject, policy context, verdict, and timestamp.
pub fn decision_payload(record: &Value) -> Result<String, String> {
    let mut map = Mapping::new();
    for field in ["subject", "policy_context", "verdict", "timestamp"] {
        let val = record
            .get(field)
            .cloned()
            .ok_or_else(|| format!("decision record missing {field}"))?;
        map.insert(Value::String(field.into()), val);
    }
    Ok(sha256_hex("decision", &canonical_cbor(&Value::Mapping(map))?))
}

/// The payload an attester signs: the envelope with its signature
/// omitted (§5.2).
pub fn envelope_payload(envelope: &Value) -> Result<String, String> {
    let mut preimage = envelope.clone();
    if let Some(map) = preimage.as_mapping_mut() {
        map.remove("signature");
    }
    Ok(sha256_hex("envelope", &canonical_cbor(&preimage)?))
}

pub struct Satisfaction {
    pub unmet: Vec<String>,
    /// Honest downgrades, e.g. an `evidence: none` envelope (§5.2).
    pub degraded: Vec<String>,
}

/// Verify one envelope's evidence chain (§5.2, §5.3 level 4). The
/// verification code path is real; `simulated` evidence checks the
/// claimed measurement against the trusted set, exactly as a
/// hardware chain would check a quote against vendor roots, and the
/// result names the mode honestly.
pub enum EvidenceResult {
    /// Verified against a trusted measurement, with the mode named.
    Verified(String),
    /// Signature-only or otherwise weakened; the reason travels.
    Degraded(String),
    Failed(String),
}

pub fn verify_evidence(envelope: &Value, trusted_measurements: &[String]) -> EvidenceResult {
    match get_str(envelope, "evidence_type") {
        Some("none") | None => EvidenceResult::Degraded(
            "evidence is none — the envelope proves who signed, not what code ran (§5.2)"
                .into(),
        ),
        Some("simulated") => {
            let measurement = envelope
                .get("evidence")
                .and_then(|e| get_str(e, "measurement"))
                .unwrap_or("");
            if measurement.is_empty() {
                EvidenceResult::Failed("simulated evidence carries no measurement".into())
            } else if trusted_measurements.contains(&measurement.to_string()) {
                EvidenceResult::Verified(format!(
                    "simulated measurement {measurement} is trusted (development mode, \
                     Appendix A step 8)"
                ))
            } else {
                EvidenceResult::Failed(format!(
                    "simulated measurement {measurement} is not in the trusted set"
                ))
            }
        }
        Some(other) => EvidenceResult::Failed(format!("unknown evidence_type {other:?}")),
    }
}

/// Check a proposal's evidence against its checklist (§4.3 step 4).
#[allow(clippy::too_many_arguments)]
pub fn check_satisfied(
    parent: &State,
    policy: &Value,
    roots: &[String],
    cs: &Value,
    author_ref: &str,
    checklist: &Checklist,
    decisions: &[Value],
    envelopes: &[Value],
) -> Result<Satisfaction, String> {
    check_satisfied_with(
        parent, policy, roots, cs, author_ref, checklist, decisions, envelopes, &[],
    )
}

/// As `check_satisfied`, with a trusted-measurement set for
/// `simulated` evidence (graph meta supplies it).
#[allow(clippy::too_many_arguments)]
pub fn check_satisfied_with(
    parent: &State,
    policy: &Value,
    roots: &[String],
    cs: &Value,
    author_ref: &str,
    checklist: &Checklist,
    decisions: &[Value],
    envelopes: &[Value],
    trusted_measurements: &[String],
) -> Result<Satisfaction, String> {
    let (cs_hash, _, _, _) = changeset_hash(cs)?;
    let pctx = policy_context(policy)?;
    let mut unmet = Vec::new();
    let mut degraded = Vec::new();

    if checklist.root_required && !roots.contains(&author_ref.to_string()) {
        unmet.push(format!(
            "default posture is restricted: author {author_ref} must hold root authority"
        ));
    }

    for (role, quorum) in &checklist.reviewers {
        let bindings: Vec<String> = policy
            .get("roles")
            .and_then(|r| r.get(role.as_str()))
            .and_then(Value::as_sequence)
            .map(|seq| {
                seq.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        let mut approvals: BTreeSet<String> = BTreeSet::new();
        for record in decisions {
            if get_str(record, "subject") != Some(cs_hash.as_str())
                || get_str(record, "verdict") != Some("approve")
            {
                continue;
            }
            if get_str(record, "policy_context") != Some(pctx.as_str()) {
                continue;
            }
            let payload = decision_payload(record)?;
            let deciders = record
                .get("deciders")
                .and_then(Value::as_sequence)
                .cloned()
                .unwrap_or_default();
            for decider in &deciders {
                let Some(principal) = get_str(decider, "principal") else { continue };
                if !bindings.contains(&principal.to_string()) {
                    continue;
                }
                let Some(signature) = get_str(decider, "signature") else { continue };
                let Some((_, obj)) = parent.find_principal(principal) else { continue };
                let keys = obj
                    .content
                    .get("attributes")
                    .and_then(|a| a.get("keys"))
                    .and_then(Value::as_sequence)
                    .cloned()
                    .unwrap_or_default();
                let verified = keys.iter().any(|record| {
                    get_str(record, "status") == Some("active")
                        && get_str(record, "public").is_some_and(|public| {
                            sign::verify(public, &payload, signature).is_ok()
                        })
                });
                if verified {
                    approvals.insert(principal.to_string());
                }
            }
        }
        if (approvals.len() as u64) < *quorum {
            unmet.push(format!(
                "reviewers: role {role} needs quorum {quorum}, have {}",
                approvals.len()
            ));
        }
    }

    for class in &checklist.attestations {
        let mut satisfied = false;
        for envelope in envelopes {
            let statement_hash = envelope
                .get("statement")
                .and_then(|s| get_str(s, "changeset_hash"));
            if statement_hash != Some(cs_hash.as_str()) {
                continue;
            }
            let Some(attester) = get_str(envelope, "attester") else { continue };
            let Some(signature) = get_str(envelope, "signature") else { continue };
            let Some((_, obj)) = parent.find_principal(attester) else { continue };
            let payload = envelope_payload(envelope)?;
            let keys = obj
                .content
                .get("attributes")
                .and_then(|a| a.get("keys"))
                .and_then(Value::as_sequence)
                .cloned()
                .unwrap_or_default();
            let verified = keys.iter().any(|record| {
                get_str(record, "public").is_some_and(|public| {
                    sign::verify(public, &payload, signature).is_ok()
                })
            });
            if !verified {
                continue;
            }
            match verify_evidence(envelope, trusted_measurements) {
                EvidenceResult::Verified(note) => {
                    satisfied = true;
                    degraded.push(format!("attestation for class {class}: {note}"));
                }
                EvidenceResult::Degraded(note) => {
                    satisfied = true;
                    degraded.push(format!("attestation for class {class}: {note}"));
                }
                EvidenceResult::Failed(_) => {}
            }
        }
        if !satisfied {
            unmet.push(format!(
                "attestation_required: no verifiable envelope from class {class}"
            ));
        }
    }

    Ok(Satisfaction { unmet, degraded })
}

// ─── tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fold::{Obj, State};
    use crate::meta::meta_registry;
    use crate::model::revision_hash;

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

    /// Build a minimal state with an owner principal so `find_principal` works.
    fn state_with_owner() -> State {
        let mut state = State::default();
        let mut user_attrs = serde_yaml::Mapping::new();
        user_attrs.insert(s("display_name"), s("owner"));
        user_attrs.insert(s("keys"), Value::Sequence(vec![]));
        user_attrs.insert(s("status"), s("active"));
        let content = mk(&[
            ("kind", s("node")),
            ("id", s("user-owner")),
            ("type", s("core/User@1")),
            ("attributes", Value::Mapping(user_attrs)),
        ]);
        let rev = revision_hash(&content).unwrap();
        state.objects.insert(
            ("node".to_string(), "user-owner".to_string()),
            Obj { content, rev, deleted: false, redacted: false },
        );
        state
    }

    /// Build a test policy doc with one rule keyed on `operation: [define-type]`.
    fn policy_with_define_type_rule() -> Value {
        serde_yaml::from_str(r#"
policy: test-policy
version: 1
default_posture: permissive
roles: {}
rules:
  - name: schema-changes-gated
    select:
      operation: [define-type, set-policy, deprecate-term]
    require:
      reviewers: { role: owner, quorum: 1 }
"#).unwrap()
    }

    /// A create op for a meta/EntityType node.
    fn entity_type_create_op() -> Value {
        let mut attrs = serde_yaml::Mapping::new();
        attrs.insert(s("name"), s("Widget"));
        attrs.insert(s("package"), s("myapp"));
        attrs.insert(s("definition"), s("attributes: {}"));
        mk(&[("create", mk(&[
            ("kind", s("node")),
            ("id", s("meta-widget-1")),
            ("type", s("meta/EntityType@1")),
            ("attributes", Value::Mapping(attrs)),
        ]))])
    }

    /// A create op for a meta/Policy node.
    fn policy_create_op() -> Value {
        let mut attrs = serde_yaml::Mapping::new();
        attrs.insert(s("name"), s("my-policy"));
        attrs.insert(s("definition"), s("{ default_posture: restricted, roles: {}, rules: [] }"));
        mk(&[("create", mk(&[
            ("kind", s("node")),
            ("id", s("meta-policy-1")),
            ("type", s("meta/Policy@1")),
            ("attributes", Value::Mapping(attrs)),
        ]))])
    }

    /// A create op for a meta/TaxonomyTerm node with status deprecated.
    fn deprecated_term_create_op() -> Value {
        let mut attrs = serde_yaml::Mapping::new();
        attrs.insert(s("name"), s("old/term"));
        attrs.insert(s("taxonomy"), s("old"));
        attrs.insert(s("parents"), Value::Sequence(vec![]));
        attrs.insert(s("status"), s("deprecated"));
        mk(&[("create", mk(&[
            ("kind", s("node")),
            ("id", s("meta-term-old")),
            ("type", s("meta/TaxonomyTerm@1")),
            ("attributes", Value::Mapping(attrs)),
        ]))])
    }

    /// A create op for a non-meta node (should NOT match define-type).
    fn user_create_op() -> Value {
        let mut attrs = serde_yaml::Mapping::new();
        attrs.insert(s("display_name"), s("alice"));
        attrs.insert(s("keys"), Value::Sequence(vec![]));
        mk(&[("create", mk(&[
            ("kind", s("node")),
            ("id", s("user-alice")),
            ("type", s("core/User@1")),
            ("attributes", Value::Mapping(attrs)),
        ]))])
    }

    fn raw_cs(ops: Vec<Value>) -> Value {
        mk(&[
            ("kind", s("changeset")),
            ("parents", Value::Sequence(vec![])),
            ("operations", Value::Sequence(ops)),
        ])
    }

    // ── Tests for sugar-verb mapping (§3.2.2) ───────────────────────────────

    /// A create op on meta/EntityType must match `operation: define-type`.
    #[test]
    fn entity_type_create_matches_define_type_selector() {
        let reg = meta_registry();
        let state = state_with_owner();
        let policy = policy_with_define_type_rule();
        let cs = raw_cs(vec![entity_type_create_op()]);

        let checklist = evaluate(&reg, &policy, &state, &cs, "user").unwrap();
        assert!(
            checklist.matched_rules.contains("schema-changes-gated"),
            "entity type create must match the define-type rule, matched: {:?}",
            checklist.matched_rules
        );
    }

    /// A create op on meta/Policy must match `operation: set-policy`.
    #[test]
    fn policy_create_matches_set_policy_selector() {
        let reg = meta_registry();
        let state = state_with_owner();
        let policy = policy_with_define_type_rule();
        let cs = raw_cs(vec![policy_create_op()]);

        let checklist = evaluate(&reg, &policy, &state, &cs, "user").unwrap();
        assert!(
            checklist.matched_rules.contains("schema-changes-gated"),
            "policy create must match the set-policy rule, matched: {:?}",
            checklist.matched_rules
        );
    }

    /// A create op on meta/TaxonomyTerm with status=deprecated must match `operation: deprecate-term`.
    #[test]
    fn deprecated_term_create_matches_deprecate_term_selector() {
        let reg = meta_registry();
        let state = state_with_owner();
        let policy = policy_with_define_type_rule();
        let cs = raw_cs(vec![deprecated_term_create_op()]);

        let checklist = evaluate(&reg, &policy, &state, &cs, "user").unwrap();
        assert!(
            checklist.matched_rules.contains("schema-changes-gated"),
            "deprecated term create must match the deprecate-term rule, matched: {:?}",
            checklist.matched_rules
        );
    }

    /// A create op for a non-meta type must NOT match `operation: define-type`.
    #[test]
    fn non_meta_create_does_not_match_define_type_selector() {
        let reg = meta_registry();
        let state = state_with_owner();
        let policy = policy_with_define_type_rule();
        let cs = raw_cs(vec![user_create_op()]);

        let checklist = evaluate(&reg, &policy, &state, &cs, "user").unwrap();
        assert!(
            !checklist.matched_rules.contains("schema-changes-gated"),
            "non-meta create must NOT match define-type rule, matched: {:?}",
            checklist.matched_rules
        );
    }

    /// A create on meta/TaxonomyTerm WITHOUT deprecated status must NOT match deprecate-term.
    #[test]
    fn active_term_create_does_not_match_deprecate_term() {
        let reg = meta_registry();
        let state = state_with_owner();
        let policy = policy_with_define_type_rule();

        let mut attrs = serde_yaml::Mapping::new();
        attrs.insert(s("name"), s("active/term"));
        attrs.insert(s("taxonomy"), s("active"));
        attrs.insert(s("parents"), Value::Sequence(vec![]));
        // No status field — active is the default
        let active_term_op = mk(&[("create", mk(&[
            ("kind", s("node")),
            ("id", s("meta-term-active")),
            ("type", s("meta/TaxonomyTerm@1")),
            ("attributes", Value::Mapping(attrs)),
        ]))]);
        let cs = raw_cs(vec![active_term_op]);

        let checklist = evaluate(&reg, &policy, &state, &cs, "user").unwrap();
        assert!(
            !checklist.matched_rules.contains("schema-changes-gated"),
            "active term create must NOT match deprecate-term rule, matched: {:?}",
            checklist.matched_rules
        );
    }

    /// A literal `operation: create` selector still matches a create verb directly.
    #[test]
    fn literal_create_verb_still_matches() {
        let reg = meta_registry();
        let state = state_with_owner();
        let policy: Value = serde_yaml::from_str(r#"
policy: test-policy
version: 1
default_posture: permissive
roles: {}
rules:
  - name: any-create
    select: { operation: create }
    require: { schema_valid: true }
"#).unwrap();
        let cs = raw_cs(vec![user_create_op()]);

        let checklist = evaluate(&reg, &policy, &state, &cs, "user").unwrap();
        assert!(
            checklist.matched_rules.contains("any-create"),
            "literal create verb selector must still match, matched: {:?}",
            checklist.matched_rules
        );
    }

    /// A schema change (define-type op) targeting workspace/scratch must NOT be
    /// exempted by scratch-is-free: the schema-changes-are-serious rule fires
    /// first and the requirements union means reviewers is still non-empty.
    ///
    /// This is the "scratch carve-out schema smuggle" attack: an agent smuggles a
    /// meta/EntityType create into the scratch region hoping the scratch-is-free
    /// carve-out drops the reviewer gate. The `evaluate()` loop unions all matched
    /// rules, so the schema rule's reviewer requirement survives even when
    /// scratch-is-free also matches and contributes only `schema_valid: true`.
    #[test]
    fn scratch_schema_smuggle_is_held() {
        let reg = meta_registry();
        let state = State::default();

        // Policy mirrors the memory-local structure: schema rule first,
        // then scratch-is-free (restricted carve-out for agents in scratch),
        // then the general agent-writes-are-proposals rule.
        let policy: Value = serde_yaml::from_str(r#"
policy: test-smuggle
version: 1
default_posture: restricted
roles:
  owner: ["principal:owner-1"]
rules:
  - name: schema-changes-are-serious
    select:
      operation: [define-type, set-policy, deprecate-term]
    require:
      reviewers: { role: owner, quorum: 1 }

  - name: scratch-is-free
    select:
      all:
        - { author_kind: agent }
        - { region: "workspace/scratch" }
    require: { schema_valid: true }

  - name: agent-writes-are-proposals
    select:
      all:
        - { author_kind: agent }
        - { not: { region: "workspace/scratch" } }
    require:
      reviewers: { role: owner, quorum: 1 }
"#).unwrap();

        // A changeset with:
        //  1. A meta/EntityType create op (schema change, sugar-verb: define-type)
        //  2. A classification op targeting workspace/scratch for that same entity
        //
        // The attacker hopes that the scratch classification causes scratch-is-free
        // to match and the reviewer requirement to be dropped. But evaluate() unions
        // all matched rules, so schema-changes-are-serious still contributes its
        // reviewer requirement.
        let mut entity_attrs = serde_yaml::Mapping::new();
        entity_attrs.insert(s("name"), s("ScratchWidget"));
        entity_attrs.insert(s("package"), s("smuggle"));
        entity_attrs.insert(s("definition"), s("attributes: {}"));
        let entity_create_op = mk(&[("create", mk(&[
            ("kind", s("node")),
            ("id", s("meta-scratch-widget")),
            ("type", s("meta/EntityType@1")),
            ("attributes", Value::Mapping(entity_attrs)),
        ]))]);

        // Classification op puts the new entity into workspace/scratch.
        let classify_op = mk(&[("create", mk(&[
            ("kind", s("classification")),
            ("id", s("classify-scratch-widget")),
            ("subject", s("node:meta-scratch-widget")),
            ("term", s("workspace/scratch")),
        ]))]);

        let cs = raw_cs(vec![entity_create_op, classify_op]);

        let checklist = evaluate(&reg, &policy, &state, &cs, "agent").unwrap();

        assert!(
            !checklist.reviewers.is_empty(),
            "schema smuggle via scratch must still require reviewers; checklist: {:?}",
            checklist
        );
        assert!(
            checklist.matched_rules.contains("schema-changes-are-serious"),
            "schema-changes-are-serious must have fired; matched rules: {:?}",
            checklist.matched_rules
        );
    }

    /// A delete op targeting a live meta/EntityType node must match
    /// `operation: define-type` by resolving the type from parent state (§3.2.2).
    #[test]
    fn delete_meta_entity_type_matches_define_type() {
        let reg = meta_registry();
        let policy = policy_with_define_type_rule();

        // Build a state that has a live meta/EntityType node.
        let mut state = state_with_owner();
        let mut attrs = serde_yaml::Mapping::new();
        attrs.insert(s("name"), s("Widget"));
        attrs.insert(s("package"), s("myapp"));
        attrs.insert(s("definition"), s("attributes: {}"));
        let et_content = mk(&[
            ("kind", s("node")),
            ("id", s("meta-widget-1")),
            ("type", s("meta/EntityType@1")),
            ("attributes", Value::Mapping(attrs)),
        ]);
        let et_rev = revision_hash(&et_content).unwrap();
        state.objects.insert(
            ("node".to_string(), "meta-widget-1".to_string()),
            crate::fold::Obj {
                content: et_content,
                rev: et_rev.clone(),
                deleted: false,
                redacted: false,
            },
        );

        // Delete op carries no `type:` field — only kind + id + prior.
        let delete_op = mk(&[("delete", mk(&[
            ("kind", s("node")),
            ("id", s("meta-widget-1")),
            ("prior", s(&et_rev)),
        ]))]);
        let cs = raw_cs(vec![delete_op]);

        let checklist = evaluate(&reg, &policy, &state, &cs, "user").unwrap();
        assert!(
            checklist.matched_rules.contains("schema-changes-gated"),
            "delete of meta/EntityType must match the define-type rule; matched: {:?}",
            checklist.matched_rules
        );
    }

    // ---- git evaluation (§3.3 selectors) ----

    fn git_policy() -> Value {
        serde_yaml::from_str(
            r#"
policy: repo-policy
version: 1
default_posture: restricted
roles:
  code-owner: [ "principal:conner" ]
  security:   [ "principal:conner" ]
rules:
  - name: main-requires-review
    select: { substrate: git, repo: "allod", target_ref: "refs/heads/main" }
    require:
      reviewers: { role: code-owner, quorum: 1 }
  - name: workflows-need-security
    select: { substrate: git, repo: "*", target_ref: "refs/heads/*", path: ".github/workflows/*" }
    require:
      reviewers: { role: security, quorum: 1 }
  - name: native-only-rule
    select: { type: "memory/Preference" }
    require:
      reviewers: { role: code-owner, quorum: 1 }
"#,
        )
        .unwrap()
    }

    #[test]
    fn glob_match_star_and_literal() {
        assert!(glob_match("refs/heads/main", "refs/heads/main"));
        assert!(glob_match("refs/heads/*", "refs/heads/feat/x"));
        assert!(glob_match("*", "anything/at/all"));
        assert!(glob_match(".github/workflows/*", ".github/workflows/ci.yml"));
        assert!(!glob_match("refs/heads/main", "refs/heads/dev"));
        assert!(!glob_match(".github/workflows/*", "src/lib.rs"));
        assert!(glob_match("a*c*e", "abcde"));
        assert!(!glob_match("a*c*e", "abcdf"));
    }

    #[test]
    fn evaluate_git_matches_ref_rule_and_skips_native_rules() {
        let change = GitChange {
            repo: "allod".into(),
            target_ref: "refs/heads/main".into(),
            ops: vec![("update".into(), "crates/allod-core/src/policy.rs".into())],
        };
        let cl = evaluate_git(&git_policy(), &change).unwrap();
        assert!(cl.matched_rules.contains("main-requires-review"));
        assert!(!cl.matched_rules.contains("native-only-rule"));
        assert_eq!(cl.reviewers, vec![("code-owner".to_string(), 1)]);
        assert!(!cl.root_required);
    }

    #[test]
    fn evaluate_git_path_rule_needs_a_touching_op() {
        let policy = git_policy();
        let touches = GitChange {
            repo: "allod".into(),
            target_ref: "refs/heads/feat/x".into(),
            ops: vec![("create".into(), ".github/workflows/governance.yml".into())],
        };
        let cl = evaluate_git(&policy, &touches).unwrap();
        assert!(cl.matched_rules.contains("workflows-need-security"));

        let misses = GitChange {
            repo: "allod".into(),
            target_ref: "refs/heads/feat/x".into(),
            ops: vec![("update".into(), "README.md".into())],
        };
        let cl = evaluate_git(&policy, &misses).unwrap();
        assert!(!cl.matched_rules.contains("workflows-need-security"));
        // feat branch: main rule doesn't match either.
        assert!(cl.matched_rules.is_empty());
        assert!(cl.reviewers.is_empty());
    }

    #[test]
    fn evaluate_git_unions_requirements_without_duplicates() {
        let change = GitChange {
            repo: "allod".into(),
            target_ref: "refs/heads/main".into(),
            ops: vec![
                ("update".into(), ".github/workflows/ci.yml".into()),
                ("update".into(), ".github/workflows/release-core.yml".into()),
            ],
        };
        let cl = evaluate_git(&git_policy(), &change).unwrap();
        assert!(cl.matched_rules.contains("main-requires-review"));
        assert!(cl.matched_rules.contains("workflows-need-security"));
        assert_eq!(
            cl.reviewers,
            vec![("code-owner".to_string(), 1), ("security".to_string(), 1)]
        );
    }
}
