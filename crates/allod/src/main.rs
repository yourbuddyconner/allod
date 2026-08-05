//! The allod CLI: a governed knowledge graph on one machine, at
//! L2-enforced with the plain-keypair profile (§6.4.1).
//!
//! Commands:
//!   init <dir> --owner <name> [--schema <ontologies-dir>]
//!   agent-add <dir> <name> --by <owner>
//!   note <dir> --as <agent> <content…>
//!   propose-preference <dir> --as <agent> --statement <s>
//!       [--strength hard|soft] [--from-note <id>]
//!   proposals <dir>
//!   approve <dir> <proposal-hash> --as <principal>
//!   log <dir> | show <dir> | verify <dir>
//!   demo [dir] [--schema <ontologies-dir>]
//!
//! The demo runs the whole jarvis flow from the memory package: a
//! free scratch note, a governed preference promotion, and a full
//! verification pass.

mod fed;
mod gitcmd;
mod md;
mod repo;

use allod_core::get_str;
use allod_core::store::Graph;
use allod_graph::ops::Admission;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

pub(crate) fn short(hash: &str) -> String {
    allod_graph::ops::short(hash)
}

/// Print the outcome of an admission decision to stdout.
///
/// Byte-identical output expected by mvp.rs and demo.rs; shared across all
/// command paths so changes stay in one place.
pub(crate) fn print_admission(admission: &Admission) {
    match admission {
        Admission::Admitted { hash, matched_rules } => {
            let basis = if matched_rules.is_empty() {
                "root authority, default posture".to_string()
            } else {
                format!("rules: {}", matched_rules.join(", "))
            };
            println!("  ✓ admitted {} ({basis})", short(hash));
        }
        Admission::Held { hash, checklist } => {
            println!("  ⧗ held as proposal {}", short(hash));
            println!(
                "      matched rules: {}",
                checklist.matched_rules.join(", ")
            );
            for (role, quorum) in &checklist.reviewers {
                println!("      requires: reviewers role {role} (quorum {quorum})");
            }
            for class in &checklist.attestations {
                println!("      requires: attestation from class {class}");
            }
            if checklist.root_required {
                println!("      requires: root authority (default posture)");
            }
        }
    }
}

// ---------------- commands ----------------

fn cmd_init(dir: &Path, owner: &str, schema_dir: &Path) -> Result<(), String> {
    cmd_init_profile(dir, owner, schema_dir, "memory")
}

fn cmd_init_profile(
    dir: &Path,
    owner: &str,
    schema_dir: &Path,
    profile: &str,
) -> Result<(), String> {
    let graph = Graph::create(dir)?;
    let profile_src = allod_graph::flows::profile_from_dir(profile, schema_dir)
        .map_err(|e| e.to_string())?;
    let result = allod_graph::flows::init(&graph, owner, profile_src)
        .map_err(|e| e.to_string())?;
    println!("  ✓ graph {} (owner {})", allod_graph::ops::short(&result.graph_id), result.owner);
    Ok(())
}

fn cmd_principal_add(dir: &Path, name: &str, kind: &str, by: &str) -> Result<(), String> {
    let graph = Graph::open(dir)?;
    let result = allod_graph::flows::principal_add(&graph, name, kind, by)
        .map_err(|e| e.to_string())?;
    print_admission(&result.admission);
    Ok(())
}

fn cmd_agent_add(dir: &Path, name: &str, by: &str) -> Result<(), String> {
    cmd_principal_add(dir, name, "agent", by)
}


fn cmd_note(dir: &Path, agent: &str, content: &str) -> Result<(String, bool), String> {
    let graph = Graph::open(dir)?;
    let result = allod_graph::flows::note(&graph, agent, content)
        .map_err(|e| e.to_string())?;
    let admitted = matches!(&result.admission, Admission::Admitted { .. });
    print_admission(&result.admission);
    Ok((result.note_id, admitted))
}

fn cmd_propose_preference(
    dir: &Path,
    agent: &str,
    statement: &str,
    strength: &str,
    from_note: Option<&str>,
) -> Result<String, String> {
    let graph = Graph::open(dir)?;
    let result = allod_graph::flows::propose_preference(&graph, agent, statement, strength, from_note)
        .map_err(|e| e.to_string())?;
    print_admission(&result.admission);
    Ok(result.hash)
}

fn cmd_approve(dir: &Path, hash: &str, by: &str) -> Result<(), String> {
    cmd_decide(dir, hash, by, "approve")
}

fn cmd_reject(dir: &Path, hash: &str, by: &str) -> Result<(), String> {
    cmd_decide(dir, hash, by, "reject")
}

fn cmd_decide(dir: &Path, hash: &str, by: &str, verdict: &str) -> Result<(), String> {
    use allod_graph::flows::DecisionOutcome;
    let graph = Graph::open(dir)?;
    let outcome = allod_graph::flows::decide(&graph, hash, by, verdict)
        .map_err(|e| e.to_string())?;
    match outcome {
        DecisionOutcome::Rejected => {
            println!("  ✗ rejected {} — proposal and decision record stay auditable",
                allod_graph::ops::short(hash));
        }
        DecisionOutcome::StillUnmet { unmet } => {
            println!("  ⧗ decision recorded; still unmet:");
            for item in &unmet {
                println!("      - {item}");
            }
        }
        DecisionOutcome::Admitted { degraded } => {
            println!("  ✓ approved and admitted {}", allod_graph::ops::short(hash));
            for note in &degraded {
                println!("      degraded: {note}");
            }
        }
    }
    Ok(())
}

fn cmd_classify(
    dir: &Path,
    node_id: &str,
    term: &str,
    by: &str,
    basis: &str,
) -> Result<Option<String>, String> {
    let graph = Graph::open(dir)?;
    let admission = allod_graph::flows::classify(&graph, node_id, term, by, basis)
        .map_err(|e| e.to_string())?;
    let held_hash = match &admission {
        Admission::Held { hash, .. } => Some(hash.clone()),
        Admission::Admitted { .. } => None,
    };
    print_admission(&admission);
    Ok(held_hash)
}

fn cmd_checkpoint(dir: &Path, by: &str) -> Result<(), String> {
    let graph = Graph::open(dir)?;
    let result = allod_graph::flows::checkpoint(&graph, by).map_err(|e| e.to_string())?;
    println!(
        "  ✓ checkpoint at {} (state {})",
        allod_graph::ops::short(&result.revision),
        allod_graph::ops::short(&result.state_hash),
    );
    Ok(())
}

fn cmd_trust(dir: &Path, measurement: &str) -> Result<(), String> {
    let graph = Graph::open(dir)?;
    allod_graph::flows::trust(&graph, measurement).map_err(|e| e.to_string())?;
    println!("  ✓ trusting simulated measurement {}", short(measurement));
    Ok(())
}

/// Emit and verify one attestation envelope for an admitted
/// changeset (Appendix A step 8). Evidence is a simulated
/// measurement; the verification code path is the real one.
fn cmd_envelope(dir: &Path, cs_hash: &str, by: &str, tool: &str) -> Result<(), String> {
    use allod_graph::flows::EnvelopeOutcome;
    let graph = Graph::open(dir)?;
    let outcome = allod_graph::flows::envelope(&graph, cs_hash, by, tool)
        .map_err(|e| e.to_string())?;
    match outcome {
        EnvelopeOutcome::Verified(note) => println!("  ✓ envelope verified: {note}"),
        EnvelopeOutcome::Degraded(note) => println!("  ⚠ envelope degraded: {note}"),
    }
    Ok(())
}

fn cmd_proposals(dir: &Path) -> Result<(), String> {
    let graph = Graph::open(dir)?;
    let list = allod_graph::flows::proposals(&graph).map_err(|e| e.to_string())?;
    if list.is_empty() {
        println!("  no pending proposals");
    }
    for p in &list {
        println!("  {}  {}  by {}", short(&p.hash), p.intent, p.author);
    }
    Ok(())
}

fn cmd_log(dir: &Path) -> Result<(), String> {
    let graph = Graph::open(dir)?;
    let entries = allod_graph::flows::log(&graph).map_err(|e| e.to_string())?;
    for e in &entries {
        println!(
            "  {}  {}  [{} op{}]  {}",
            short(&e.hash),
            e.author,
            e.op_count,
            if e.op_count == 1 { "" } else { "s" },
            e.intent
        );
    }
    Ok(())
}

fn cmd_show(dir: &Path) -> Result<(), String> {
    let graph = Graph::open(dir)?;
    let view = allod_graph::flows::state(&graph).map_err(|e| e.to_string())?;
    println!("  state hash {}", short(&view.state_hash));
    for node in &view.nodes {
        let by = node.derived_by.as_deref()
            .map(|p| format!("  (by {p})"))
            .unwrap_or_default();
        println!("  {}  {}{}", node.type_ref, node.label, by);
    }
    Ok(())
}

fn cmd_verify(dir: &Path) -> Result<(), String> {
    use allod_graph::flows::LevelResult;
    let graph = Graph::open(dir)?;
    let report = allod_graph::flows::verify(&graph).map_err(|e| e.to_string())?;
    let chain_len = report.changesets.len();

    for cs_entry in &report.changesets {
        // Check for failures before printing — must not print ✓ on the failure path.
        // Check governance first to surface real failure reasons (not "not reached" sentinels).
        match &cs_entry.governance {
            LevelResult::Failed(r) => return Err(r.clone()),
            _ => {}
        }
        match &cs_entry.integrity {
            LevelResult::Failed(r) => return Err(r.clone()),
            _ => {}
        }
        match &cs_entry.authorship {
            LevelResult::Failed(r) => return Err(r.clone()),
            _ => {}
        }
        let author_ref = &cs_entry.author;
        let admitted_by = &cs_entry.admitted_by;
        println!(
            "  ✓ {}  integrity ✓  authorship ✓ ({author_ref})  governance: {admitted_by}",
            short(&cs_entry.hash)
        );
    }
    println!("  state hash {}", short(&report.state_hash));
    for cp in &report.checkpoints {
        // Check cp.ok before printing — must not print ✓ on the failure path
        if !cp.ok {
            let reason = cp.reason.as_deref().unwrap_or("checkpoint verification failed");
            return Err(reason.to_string());
        }
        println!(
            "  ✓ checkpoint {} verified against replay ({})",
            short(&cp.revision),
            cp.signer
        );
    }
    for note in &report.degraded {
        println!("  ⚠ degraded: {note}");
    }
    println!(
        "  VERIFIED: {chain_len} changesets, levels 1-3 (§5.3){}",
        if report.degraded.is_empty() { "" } else { ", with degradations noted" }
    );
    if !report.ok {
        return Err("verify failed".into());
    }
    Ok(())
}

// ---------------- the demo ----------------

fn cmd_demo(dir: &Path, schema_dir: &Path) -> Result<(), String> {
    println!("allod agent-memory demo — the jarvis flow (ontologies/memory)");
    println!("═══════════════════════════════════════════════════════════════");
    println!("\n[1/7] Genesis: conner creates a graph, memory-local policy, restricted posture");
    cmd_init(dir, "conner", schema_dir)?;

    println!("\n[2/7] Register the agent: jarvis, delegated by conner (§6.1)");
    cmd_agent_add(dir, "jarvis", "conner")?;

    println!("\n[3/7] Jarvis writes a scratch note — thinks at full speed (rule scratch-is-free)");
    let (note_id, admitted) = cmd_note(
        dir,
        "jarvis",
        "Conner declined 4 of the last 5 meetings before 09:00, all with a reschedule note.",
    )?;
    if !admitted {
        return Err("scratch note should have admitted freely".into());
    }

    println!("\n[4/7] Jarvis proposes a Preference — held for the owner (§4.3)");
    let hash = cmd_propose_preference(
        dir,
        "jarvis",
        "No meetings before 09:00",
        "soft",
        Some(&note_id),
    )?;

    println!("\n[5/7] Conner approves — a signed, portable decision record admits it");
    cmd_approve(dir, &hash, "conner")?;

    println!("\n[6/7] Verify from the log and public keys alone (§5.3)");
    cmd_verify(dir)?;

    println!("\n[7/7] The memory:");
    cmd_show(dir)?;

    println!("\nThe sovereign-memory claim, demonstrated: the preference exists");
    println!("because conner signed, and any host — or no host — can verify that.");
    Ok(())
}

/// Federation demo (Appendix A step 9): two sovereign graphs, a
/// grant, a proofs-carrying bundle by file copy, a governed import,
/// and revocation.
fn cmd_demo_federation(dir_a: &Path, dir_b: &Path, schema: &Path) -> Result<(), String> {
    println!("allod federation demo — two sovereign graphs, one byte channel (a file)");
    println!("═══════════════════════════════════════════════════════════════════════");
    println!("\n[1/8] Graph A: conner + jarvis build the memory (the jarvis flow, quietly)");
    cmd_init(dir_a, "conner", schema)?;
    cmd_agent_add(dir_a, "jarvis", "conner")?;
    let (note_id, _) = cmd_note(
        dir_a,
        "jarvis",
        "Conner declined 4 of the last 5 meetings before 09:00.",
    )?;
    let hash = cmd_propose_preference(
        dir_a, "jarvis", "No meetings before 09:00", "soft", Some(&note_id),
    )?;
    cmd_approve(dir_a, &hash, "conner")?;

    println!("\n[2/8] Graph B: dana's own graph, its own root authority (§9.1)");
    cmd_init(dir_b, "dana", schema)?;

    println!("\n[3/8] B learns A's identity out of band: a peer record (§9.3)");
    let graph_a = Graph::open(dir_a)?;
    let a_id = get_str(&graph_a.meta()?, "graph_id").unwrap_or("").to_string();
    let a_key = graph_a.load_key("conner")?.public_hex();
    fed::peer_add(dir_b, "conner-memory", &a_id, &a_key, "dana")?;

    println!("\n[4/8] A issues a grant: region work, audience = B's graph ID (§9.4)");
    let graph_b = Graph::open(dir_b)?;
    let b_id = get_str(&graph_b.meta()?, "graph_id").unwrap_or("").to_string();
    let grant_id = fed::grant(dir_a, &b_id, "work", "conner")?;

    println!("\n[5/8] A produces a share bundle: objects with membership proofs (§9.5)");
    let bundle_path = std::env::temp_dir().join("allod-share-bundle.yaml");
    fed::bundle(dir_a, &grant_id, &bundle_path, "conner")?;

    println!("\n[6/8] B verifies the bundle with A's keys and imports the preference (§9.6)");
    let state_a = graph_a.fold()?;
    let pref_id = state_a
        .objects
        .iter()
        .find_map(|((kind, id), obj)| {
            (kind == "node"
                && get_str(&obj.content, "type").map(allod_core::bare)
                    == Some("memory/Preference"))
            .then(|| id.clone())
        })
        .ok_or("no preference in A")?;
    let held = fed::import(dir_b, &bundle_path, "dana", Some(&pref_id))?;
    if let Some(proposal) = held {
        println!("      B's own policy held the foreign preference — receiver-governed");
        println!("      exchange (§9.6, design principle 6). Dana decides:");
        cmd_approve(dir_b, &proposal, "dana")?;
    }

    println!("\n[7/8] B verifies its own graph — the import's lineage crosses intact");
    cmd_verify(dir_b)?;

    println!("\n[8/8] A revokes the grant; the next bundle request is refused (§9.4)");
    fed::revoke(dir_a, &grant_id, "conner")?;
    match fed::bundle(dir_a, &grant_id, &bundle_path, "conner") {
        Err(e) => println!("  ✓ refused: {e}"),
        Ok(()) => return Err("bundle should be refused after revocation".into()),
    }
    println!("\nBoth graphs stayed sovereign: B admitted under its own policy, and");
    println!("the claim in B still proves where it came from, by hash, forever.");
    Ok(())
}

/// Code-graph demo (Appendix A steps 2-4 and 8): commit-aligned
/// derivation, governed classification, the semantic diff with blast
/// radius, and one attested (simulated) envelope.
fn cmd_demo_code(dir: &Path, schema: &Path) -> Result<(), String> {
    println!("allod code-graph demo — a repository as governed knowledge (§8.3)");
    println!("═══════════════════════════════════════════════════════════════════");
    let repo_dir = dir.with_extension("repo");
    println!("\n[1/8] A sample repository: two commits on a spend path");
    let (first, second) = repo::make_sample_repo(&repo_dir)?;
    println!("  ✓ {} then {}", short(&first), short(&second));

    println!("\n[2/8] Genesis: code profile — core + code ontologies, eng taxonomy");
    cmd_init_profile(dir, "conner", schema, "code")?;
    cmd_principal_add(dir, "indexer", "service", "conner")?;
    cmd_principal_add(dir, "maria", "user", "conner")?;

    println!("\n[3/8] Import commit one — deterministic indexer writes admit (§8.1)");
    let (_h1, admitted) = repo::import_commit(dir, &repo_dir, &first, "indexer")?;
    if !admitted {
        return Err("first import should admit under the deterministic rule".into());
    }

    println!("\n[4/8] Maria proposes: authorize_spend is security/critical (Appendix A step 4)");
    let graph = Graph::open(dir)?;
    let state = graph.fold()?;
    let fn_id = state
        .objects
        .iter()
        .find_map(|((kind, id), obj)| {
            (kind == "node"
                && obj.content.get("attributes").and_then(|a| get_str(a, "name"))
                    == Some("authorize_spend"))
            .then(|| id.clone())
        })
        .ok_or("authorize_spend not derived")?;
    let held = cmd_classify(dir, &fn_id, "security/critical@1", "maria", "manual")?;
    let proposal = held.ok_or("classification should be held for the owner")?;
    println!("      the owner signs off:");
    cmd_approve(dir, &proposal, "conner")?;

    println!("\n[5/8] Import commit two — it touches classified code, so it is held (§8.3)");
    let (h2, admitted) = repo::import_commit(dir, &repo_dir, &second, "indexer")?;
    if admitted {
        return Err("second import touches security/critical and must be held".into());
    }

    println!("\n[6/8] The semantic diff, as a review artifact (§4.4): what changed, who calls it");
    repo::semantic_diff(dir, &h2, None)?;
    println!("      reviewed; the owner admits it:");
    cmd_approve(dir, &h2, "conner")?;

    println!("\n[7/8] One attested envelope, simulated evidence, real verification (step 8)");
    let measurement = allod_core::hash::plain_sha256(repo::SCAN_TOOL.as_bytes());
    cmd_trust(dir, &measurement)?;
    cmd_envelope(dir, &h2, "indexer", repo::SCAN_TOOL)?;

    println!("\n[8/8] Verify the whole graph from the log and keys");
    cmd_verify(dir)?;
    Ok(())
}

// ---------------- argument plumbing ----------------

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

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().cloned().unwrap_or_default();
    let rest = &args[1.min(args.len())..];
    let pos = positional(rest);
    let dir = pos.first().map(PathBuf::from);
    let schema = flag(rest, "--schema").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from("ontologies")
    });

    let result = match command.as_str() {
        "init" => match (dir, flag(rest, "--owner")) {
            (Some(dir), Some(owner)) => cmd_init(&dir, &owner, &schema),
            _ => Err("usage: allod init <dir> --owner <name> [--schema <dir>]".into()),
        },
        "agent-add" => match (dir, pos.get(1), flag(rest, "--by")) {
            (Some(dir), Some(name), Some(by)) => cmd_agent_add(&dir, name, &by),
            _ => Err("usage: allod agent-add <dir> <name> --by <owner>".into()),
        },
        "principal-add" => match (dir, pos.get(1), flag(rest, "--kind"), flag(rest, "--by")) {
            (Some(dir), Some(name), Some(kind), Some(by)) => {
                cmd_principal_add(&dir, name, &kind, &by)
            }
            _ => Err("usage: allod principal-add <dir> <name> --kind user|service|agent --by <owner>".into()),
        },
        "classify" => match (dir, pos.get(1), pos.get(2), flag(rest, "--as")) {
            (Some(dir), Some(node), Some(term), Some(by)) => cmd_classify(
                &dir, node, term, &by,
                &flag(rest, "--basis").unwrap_or_else(|| "manual".into()),
            ).map(|_| ()),
            _ => Err("usage: allod classify <dir> <node-id> <term> --as <principal> [--basis b]".into()),
        },
        "reject" => match (dir, pos.get(1), flag(rest, "--as")) {
            (Some(dir), Some(hash), Some(by)) => cmd_reject(&dir, hash, &by),
            _ => Err("usage: allod reject <dir> <proposal-hash> --as <principal>".into()),
        },
        "checkpoint" => match (dir, flag(rest, "--as")) {
            (Some(dir), Some(by)) => cmd_checkpoint(&dir, &by),
            _ => Err("usage: allod checkpoint <dir> --as <principal>".into()),
        },
        "trust" => match (dir, pos.get(1)) {
            (Some(dir), Some(m)) => cmd_trust(&dir, m),
            _ => Err("usage: allod trust <dir> <measurement-hash>".into()),
        },
        "envelope" => match (dir, pos.get(1), flag(rest, "--as"), flag(rest, "--tool")) {
            (Some(dir), Some(hash), Some(by), Some(tool)) => {
                cmd_envelope(&dir, hash, &by, &tool)
            }
            _ => Err("usage: allod envelope <dir> <cs-hash> --as <principal> --tool <identity>".into()),
        },
        "note" => match (dir, flag(rest, "--as")) {
            (Some(dir), Some(agent)) => {
                let content = pos[1..].join(" ");
                cmd_note(&dir, &agent, &content).map(|_| ())
            }
            _ => Err("usage: allod note <dir> --as <agent> <content…>".into()),
        },
        "propose-preference" => match (dir, flag(rest, "--as"), flag(rest, "--statement")) {
            (Some(dir), Some(agent), Some(statement)) => cmd_propose_preference(
                &dir,
                &agent,
                &statement,
                &flag(rest, "--strength").unwrap_or_else(|| "soft".into()),
                flag(rest, "--from-note").as_deref(),
            )
            .map(|_| ()),
            _ => Err(
                "usage: allod propose-preference <dir> --as <agent> --statement <s> \
                 [--strength hard|soft] [--from-note <id>]"
                    .into(),
            ),
        },
        "approve" => match (dir, pos.get(1), flag(rest, "--as")) {
            (Some(dir), Some(hash), Some(by)) => cmd_approve(&dir, hash, &by),
            _ => Err("usage: allod approve <dir> <proposal-hash> --as <principal>".into()),
        },
        "peer-add" => match (dir, pos.get(1), flag(rest, "--graph-id"), flag(rest, "--key"), flag(rest, "--by")) {
            (Some(dir), Some(name), Some(gid), Some(key), Some(by)) => {
                fed::peer_add(&dir, name, &gid, &key, &by)
            }
            _ => Err("usage: allod peer-add <dir> <name> --graph-id <hash> --key <public-hex> --by <principal>".into()),
        },
        "grant" => match (dir, flag(rest, "--audience"), flag(rest, "--region"), flag(rest, "--as")) {
            (Some(dir), Some(aud), Some(region), Some(by)) => {
                fed::grant(&dir, &aud, &region, &by).map(|id| println!("  grant id {id}"))
            }
            _ => Err("usage: allod grant <dir> --audience <graph-id|public> --region <term> --as <principal>".into()),
        },
        "grant-revoke" => match (dir, pos.get(1), flag(rest, "--as")) {
            (Some(dir), Some(id), Some(by)) => fed::revoke(&dir, id, &by),
            _ => Err("usage: allod grant-revoke <dir> <grant-id> --as <principal>".into()),
        },
        "bundle" => match (dir, pos.get(1), pos.get(2), flag(rest, "--as")) {
            (Some(dir), Some(grant_id), Some(out), Some(by)) => {
                fed::bundle(&dir, grant_id, Path::new(out), &by)
            }
            _ => Err("usage: allod bundle <dir> <grant-id> <out-file> --as <principal>".into()),
        },
        "bundle-import" => match (dir, pos.get(1), flag(rest, "--as")) {
            (Some(dir), Some(bundle), Some(by)) => fed::import(
                &dir, Path::new(bundle), &by, flag(rest, "--import").as_deref(),
            ).map(|_| ()),
            _ => Err("usage: allod bundle-import <dir> <bundle-file> --as <principal> [--import <node-id>]".into()),
        },
        "init-code" => match (dir, flag(rest, "--owner")) {
            (Some(dir), Some(owner)) => cmd_init_profile(&dir, &owner, &schema, "code"),
            _ => Err("usage: allod init-code <dir> --owner <name> [--schema <dir>]".into()),
        },
        "repo-import" => match (dir, pos.get(1), pos.get(2), flag(rest, "--as")) {
            (Some(dir), Some(repo_path), Some(commit), Some(by)) => {
                repo::import_commit(&dir, Path::new(repo_path), commit, &by).map(|_| ())
            }
            _ => Err("usage: allod repo-import <dir> <repo> <commit> --as <service>".into()),
        },
        "semantic-diff" => match (dir, pos.get(1)) {
            (Some(dir), Some(hash)) => repo::semantic_diff(
                &dir, hash, flag(rest, "--out").as_deref().map(Path::new),
            ),
            _ => Err("usage: allod semantic-diff <dir> <cs-hash> [--out <file>]".into()),
        },
        "demo-code" => {
            let dir = dir.unwrap_or_else(|| PathBuf::from("allod-demo-code"));
            cmd_demo_code(&dir, &schema)
        }
        "demo-federation" => {
            let a = dir.unwrap_or_else(|| PathBuf::from("allod-demo-a"));
            let b = pos.get(1).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("allod-demo-b"));
            cmd_demo_federation(&a, &b, &schema)
        }
        "git" => gitcmd::cmd_git(rest),
        "install-policy" => match (dir, pos.get(1), flag(rest, "--as")) {
            (Some(dir), Some(policy_file), Some(by)) => {
                gitcmd::cmd_install_policy(&dir, std::path::Path::new(policy_file), &by)
            }
            _ => Err("usage: allod install-policy <graph-dir> <policy-file> --as <principal>".into()),
        },
        "export-md" => match (dir, pos.get(1)) {
            (Some(dir), Some(out)) => md::export(&dir, Path::new(out)),
            _ => Err("usage: allod export-md <dir> <out-dir>".into()),
        },
        "import-md" => match (dir, pos.get(1), flag(rest, "--as")) {
            (Some(dir), Some(bundle), Some(by)) => md::import(&dir, Path::new(bundle), &by),
            _ => Err("usage: allod import-md <dir> <bundle-dir> --as <principal>".into()),
        },
        "proposals" => dir.ok_or("usage: allod proposals <dir>".to_string())
            .and_then(|d| cmd_proposals(&d)),
        "log" => dir.ok_or("usage: allod log <dir>".to_string()).and_then(|d| cmd_log(&d)),
        "show" => dir.ok_or("usage: allod show <dir>".to_string()).and_then(|d| cmd_show(&d)),
        "verify" => dir
            .ok_or("usage: allod verify <dir>".to_string())
            .and_then(|d| cmd_verify(&d)),
        "demo" => {
            let dir = dir.unwrap_or_else(|| PathBuf::from("allod-demo"));
            cmd_demo(&dir, &schema)
        }
        _ => Err(
            "usage: allod <init|agent-add|note|propose-preference|proposals|approve|log|show|verify|demo|git|install-policy> …"
                .into(),
        ),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
