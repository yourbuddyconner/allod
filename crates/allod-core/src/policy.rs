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
        contexts.push(OpContext {
            verb,
            kind,
            type_ref,
            basis,
            imported,
            regions,
            payload: payload.clone(),
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
        if !ops.contains(&ctx.verb.as_str()) {
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
