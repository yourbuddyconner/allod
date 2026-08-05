//! `allod git eval`, `allod git decide`, and `allod git index` — advisory governance over git commits.
//!
//! # eval
//! Resolves a commit-ish to its full SHA, builds a [`GitChange`], evaluates it
//! against the graph's policy, reads existing decision records from
//! `refs/notes/allod-decisions`, and calls `reviewers_unmet` to determine
//! whether the commit is cleared to merge. Exits 0 when all requirements are
//! satisfied (or no rule matches), 1 otherwise.
//!
//! # decide
//! Builds a signed decision record in exactly the same shape that
//! `reviewers_unmet` verifies (matching `flows::decide`), and appends it to
//! the `refs/notes/allod-decisions` note for the resolved SHA.
//!
//! # index
//! Materializes the derived graph for a commit by calling `import_commit`,
//! which scans the commit tree to extract code entities and admit or hold
//! the changeset according to the graph's policy. Idempotent: re-running
//! on the same commit reports "up to date" if nothing changed.

use allod_core::policy::{
    attach_decider, build_decision_record, decision_payload, evaluate_git, reviewers_unmet,
    Checklist, GitChange,
};
use allod_core::store::Graph;
use allod_graph::repo as repo_lib;
use allod_substrate::Substrate;
use allod_substrate_git::{append_decision, op_paths, read_decisions, resolve_commit, GitSubstrate};
use serde_yaml::Value;
use std::path::{Path, PathBuf};

// ── flag helpers (mirrors main.rs) ───────────────────────────────────────────

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn positional(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut skip = false;
    for arg in args {
        if skip {
            skip = false;
            continue;
        }
        if arg.starts_with("--") {
            skip = true;
            continue;
        }
        out.push(arg.clone());
    }
    out
}

// ── public dispatch ───────────────────────────────────────────────────────────

/// Dispatch `allod git <subcommand> …` based on the first positional.
pub fn cmd_git(args: &[String]) -> Result<(), String> {
    let sub = args.first().cloned().unwrap_or_default();
    let rest = &args[1.min(args.len())..];
    match sub.as_str() {
        "eval" => cmd_git_eval(rest),
        "decide" => cmd_git_decide(rest),
        "index" => cmd_git_index(rest),
        _ => Err(format!(
            "usage: allod git eval|decide|index …\n  unknown subcommand: {sub}"
        )),
    }
}

// ── eval ─────────────────────────────────────────────────────────────────────

fn cmd_git_eval(args: &[String]) -> Result<(), String> {
    let pos = positional(args);
    let repo_dir = pos.first().map(PathBuf::from).ok_or("usage: allod git eval <repo-dir> <commit-ish> --target <ref> [--graph <dir>] [--repo <name>] [--json]")?;
    let commitish = pos.get(1).cloned().ok_or("usage: allod git eval <repo-dir> <commit-ish> --target <ref> [--graph <dir>] [--repo <name>] [--json]")?;
    let target_ref = flag(args, "--target").ok_or("--target <ref> is required")?;

    // Defaults. --graph specifies the directory containing .allod/;
    // Graph::open appends .allod internally.
    let graph_dir = flag(args, "--graph")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_dir.to_path_buf());
    let repo_name = flag(args, "--repo").unwrap_or_else(|| {
        repo_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "repo".to_string())
    });
    let json_mode = args.iter().any(|a| a == "--json");

    // Resolve commit.
    let sha = resolve_commit(&repo_dir, &commitish)?;

    // Build operation set.
    let substrate = GitSubstrate::new(&repo_dir);
    let ops = substrate.operation_set(&format!("sha1:{sha}"))?;
    let ops_paths = op_paths(&ops);

    // Build GitChange.
    let change = GitChange {
        repo: repo_name.clone(),
        target_ref: target_ref.clone(),
        ops: ops_paths,
    };

    // Open graph and evaluate.
    let graph = Graph::open(&graph_dir)?;
    let policy = graph.policy()?;

    // Fold graph state and build registry for derived-graph region reach (§8.3)
    // and for principal lookup in reviewers_unmet.
    let state = graph.fold()?;
    let reg = graph.registry()?;
    let checklist: Checklist = evaluate_git(&policy, &change, Some((&state, &reg)))?;

    // Read decisions.
    let subject = format!("git:{sha}");
    let decisions = read_decisions(&repo_dir, &sha)?;

    let unmet = reviewers_unmet(&state, &policy, &subject, &checklist, &decisions)?;

    if json_mode {
        // Emit JSON manually (serde_json not in deps).
        let matched_rules_json = checklist
            .matched_rules
            .iter()
            .map(|r| format!("\"{}\"", escape_json(r)))
            .collect::<Vec<_>>()
            .join(",");
        let reviewers_json = checklist
            .reviewers
            .iter()
            .map(|(role, quorum)| {
                let satisfied = !unmet.iter().any(|u| u.contains(role.as_str()));
                format!(
                    "{{\"role\":\"{}\",\"quorum\":{},\"satisfied\":{}}}",
                    escape_json(role),
                    quorum,
                    satisfied
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let unmet_json = unmet
            .iter()
            .map(|u| format!("\"{}\"", escape_json(u)))
            .collect::<Vec<_>>()
            .join(",");
        let verdict = if unmet.is_empty() { "pass" } else { "fail" };
        println!(
            "{{\"commit\":\"{sha}\",\"target\":\"{target}\",\"matched_rules\":[{matched}],\"reviewers\":[{reviewers}],\"unmet\":[{unmet}],\"verdict\":\"{verdict}\"}}",
            sha = sha,
            target = escape_json(&target_ref),
            matched = matched_rules_json,
            reviewers = reviewers_json,
            unmet = unmet_json,
            verdict = verdict,
        );
    } else {
        // Human-readable output.
        println!("commit  {sha}");
        println!("target  {target_ref}");
        if checklist.matched_rules.is_empty() {
            println!("matched rules: (none — trivially pass)");
        } else {
            let rules: Vec<&str> = checklist.matched_rules.iter().map(|s| s.as_str()).collect();
            println!("matched rules: {}", rules.join(", "));
        }
        for (role, quorum) in &checklist.reviewers {
            let met = !unmet.iter().any(|u| u.contains(role.as_str()));
            let mark = if met { "✓" } else { "✗" };
            println!("  {mark} reviewers role {role} (quorum {quorum})");
        }
        if unmet.is_empty() {
            println!("verdict: pass");
        } else {
            println!("verdict: fail");
            for u in &unmet {
                println!("  unmet: {u}");
            }
        }
    }

    if unmet.is_empty() {
        Ok(())
    } else {
        Err(String::new()) // exit 1; error already printed above
    }
}

// ── decide ────────────────────────────────────────────────────────────────────

fn cmd_git_decide(args: &[String]) -> Result<(), String> {
    let pos = positional(args);
    let repo_dir = pos.first().map(PathBuf::from).ok_or("usage: allod git decide <repo-dir> <commit-ish> --as <principal> [--verdict approve|reject] [--graph <dir>]")?;
    let commitish = pos.get(1).cloned().ok_or("usage: allod git decide <repo-dir> <commit-ish> --as <principal> [--verdict approve|reject] [--graph <dir>]")?;
    let principal = flag(args, "--as").ok_or("--as <principal> is required")?;
    let verdict = flag(args, "--verdict").unwrap_or_else(|| "approve".to_string());

    // Defaults. --graph specifies the directory containing .allod/;
    // Graph::open appends .allod internally.
    let graph_dir = flag(args, "--graph")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_dir.to_path_buf());

    // Resolve commit.
    let sha = resolve_commit(&repo_dir, &commitish)?;
    let subject = format!("git:{sha}");

    // Open graph.
    let graph = Graph::open(&graph_dir)?;
    let policy = graph.policy()?;
    let signer = graph.signer(&principal).map_err(|e| e.to_string())?;

    // Build the unsigned decision record using the shared builder.
    let mut record = build_decision_record(&policy, &subject, &verdict, &allod_graph::ops::now_iso())?;

    // Sign and attach.
    let payload = decision_payload(&record)?;
    let signature = signer.sign(&payload).map_err(|e| e.to_string())?;
    attach_decider(&mut record, &principal, &signature);

    // Append to git notes.
    append_decision(&repo_dir, &sha, &record)?;

    println!("  ✓ decision recorded: {subject} → {verdict}");
    Ok(())
}

// ── index ────────────────────────────────────────────────────────────────────

fn cmd_git_index(args: &[String]) -> Result<(), String> {
    let pos = positional(args);
    let repo_dir = pos.first().map(PathBuf::from).ok_or("usage: allod git index <repo-dir> <commit-ish> --as <principal> [--graph <dir>]")?;
    let commitish = pos.get(1).cloned().ok_or("usage: allod git index <repo-dir> <commit-ish> --as <principal> [--graph <dir>]")?;
    let principal = flag(args, "--as").ok_or("--as <principal> is required")?;

    // Defaults. --graph specifies the directory containing .allod/;
    // Graph::open appends .allod internally.
    let graph_dir = flag(args, "--graph")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_dir.to_path_buf());

    // Open graph and import the commit.
    let graph = Graph::open(&graph_dir)?;
    match repo_lib::import_commit(&graph, &repo_dir, &commitish, &principal) {
        Ok((hash, admitted)) => {
            // Print admission outcome.
            let admitted_str = if admitted {
                "admitted"
            } else {
                "held"
            };
            println!(
                "  ✓ indexed {} ({}) — derivation {} at {}",
                allod_graph::ops::short(&hash),
                admitted_str,
                if admitted { "admitted" } else { "pending" },
                commitish
            );
            Ok(())
        }
        Err(e) if e.to_string().contains("nothing changed") => {
            // Resolve the commit to get its SHA for the message.
            let sha = resolve_commit(&repo_dir, &commitish)?;
            println!("  up to date — no derivation ops for {}", &sha[..12.min(sha.len())]);
            Ok(())
        }
        Err(e) => Err(e.to_string()),
    }
}

// ── JSON helpers ──────────────────────────────────────────────────────────────

/// Escape a string for JSON (handles `\`, `"`, and control chars).
fn escape_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

// ── install-policy helper (also called from main.rs) ─────────────────────────

/// Install (or update) a policy in the graph from a YAML file, signed by `by`.
///
/// Wraps `allod_graph::flows::install_package` with an empty doc set and the
/// parsed policy. Under the memory profile the policy update is held for the
/// owner's approval; the owner signs via the `decide` flow to complete
/// admission.
pub fn cmd_install_policy(graph_dir: &Path, policy_file: &Path, by: &str) -> Result<(), String> {
    use allod_graph::flows::install_package;
    use allod_graph::ops::Admission;

    let text = std::fs::read_to_string(policy_file)
        .map_err(|e| format!("read {}: {e}", policy_file.display()))?;
    let policy: Value = serde_yaml::from_str(&text).map_err(|e| e.to_string())?;

    let graph = Graph::open(graph_dir)?;
    let admission = install_package(&graph, &[], Some(&policy), by).map_err(|e| e.to_string())?;

    match &admission {
        Admission::Admitted { hash, matched_rules } => {
            let basis = if matched_rules.is_empty() {
                "root authority, default posture".to_string()
            } else {
                format!("rules: {}", matched_rules.join(", "))
            };
            println!(
                "  ✓ policy admitted {} ({basis})",
                allod_graph::ops::short(hash)
            );
        }
        Admission::Held { hash, checklist } => {
            // Auto-approve if by is the owner (the proposal is signed by the
            // same principal who would approve it — mirrors wasm tests).
            // We call the graph-level decide flow to complete admission.
            let hash = hash.clone();
            let matched = checklist.matched_rules.join(", ");
            println!("  ⧗ policy held as proposal {} (matched: {matched})", allod_graph::ops::short(&hash));
            println!("    auto-approving as owner {by}…");
            use allod_graph::flows::{decide, DecisionOutcome};
            match decide(&graph, &hash, by, "approve").map_err(|e| e.to_string())? {
                DecisionOutcome::Admitted { .. } => {
                    println!("  ✓ policy admitted (owner approved)");
                }
                DecisionOutcome::StillUnmet { unmet } => {
                    return Err(format!(
                        "policy still unmet after owner approval: {unmet:?}"
                    ));
                }
                DecisionOutcome::Rejected => {
                    return Err("policy rejected unexpectedly".into());
                }
            }
        }
    }

    Ok(())
}
