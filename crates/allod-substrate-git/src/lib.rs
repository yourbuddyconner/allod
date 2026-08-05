//! `allod-substrate-git` — `GitSubstrate` over the git CLI.
//!
//! Implements the milestone-1 [`Substrate`] trait by shelling out to the
//! `git` binary (the `repo.rs` idiom — NOT gix; gix remains an option if
//! shelling out ever binds). All revision hashes carry the `sha1:` prefix
//! (§1.7). Operation sets are derived from `git diff-tree` with rename
//! detection disabled for determinism (§3.3).
//!
//! # Decisions without rewriting commits (§3.4)
//!
//! Decision records for git commits live in
//! `refs/notes/allod-decisions`. Attaching decisions via git notes means
//! a decided commit is never rewritten — committing a decision into the
//! PR branch would change the head SHA under decision, making the
//! decision self-referentially invalid. Notes travel with the repo via
//! `git push origin refs/notes/allod-decisions` and fetched symmetrically.

use allod_substrate::{AuthorVerdict, Revision, Substrate};
use serde_yaml::{Mapping, Value};
use std::path::Path;

// ── private git helper ────────────────────────────────────────────────────────

/// Spawn `git -C <repo> <args>`, return stdout on success or Err(stderr).
///
/// Mirrors the idiom in `crates/allod-graph/src/repo.rs`.
fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|e| format!("git spawn: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

// ── GitSubstrate ──────────────────────────────────────────────────────────────

/// [`Substrate`] implementation backed by the `git` CLI.
///
/// Revision hashes are `sha1:<40-hex>`. The substrate never modifies the
/// git object store or branch tips; decision records use git notes (see
/// [`append_decision`]).
pub struct GitSubstrate {
    repo: std::path::PathBuf,
}

impl GitSubstrate {
    /// Create a substrate view over `repo_dir`.
    pub fn new(repo_dir: &Path) -> Self {
        Self { repo: repo_dir.to_path_buf() }
    }
}

impl Substrate for GitSubstrate {
    /// Return revision metadata for `hash` (must be `sha1:<40-hex>`).
    ///
    /// Validates content-addressing: `git rev-parse --verify <sha>^{commit}`
    /// must echo back the same SHA (a git object hash IS its content hash).
    /// Then fetches parents, timestamp, and author string via
    /// `git show -s --format=…`, and detects signing via
    /// `git cat-file commit <sha>` checking for a `gpgsig ` header.
    fn revision(&self, hash: &str) -> Result<Revision, String> {
        let sha = hash
            .strip_prefix("sha1:")
            .ok_or_else(|| format!("hash lacks sha1: prefix: {hash}"))?;

        // Content-addressing check.
        let resolved = git(&self.repo, &["rev-parse", "--verify", &format!("{sha}^{{commit}}")])
            .map_err(|e| format!("unknown revision {sha}: {e}"))?;
        let resolved = resolved.trim();
        if resolved != sha {
            return Err(format!(
                "revision mismatch: rev-parse returned {resolved}, expected {sha}"
            ));
        }

        // Parents, timestamp, author in one shot.
        // %P = space-separated parent SHAs (empty for root); %aI = author ISO8601; %an <%ae> = author name+email.
        let show = git(
            &self.repo,
            &["show", "-s", &format!("--format=%P%n%aI%n%an <%ae>"), sha],
        )?;
        let mut lines = show.lines();
        let parents_line = lines.next().unwrap_or("").trim().to_string();
        let timestamp_line = lines.next().map(|s| s.trim().to_string());
        let author_line = lines.next().unwrap_or("").trim().to_string();

        let parents: Vec<String> = if parents_line.is_empty() {
            vec![]
        } else {
            parents_line
                .split_whitespace()
                .map(|p| format!("sha1:{p}"))
                .collect()
        };

        let timestamp = timestamp_line.filter(|s| !s.is_empty());

        // Parse "Name <email>" into a YAML mapping { name: …, email: … }.
        let (name, email) = parse_author(&author_line);
        let mut author_map = Mapping::new();
        author_map.insert(Value::String("name".into()), Value::String(name));
        author_map.insert(Value::String("email".into()), Value::String(email));
        let author = Value::Mapping(author_map);

        // Detect gpg/ssh signature by checking for a `gpgsig ` header.
        let cat = git(&self.repo, &["cat-file", "commit", sha])?;
        let signed = cat.lines().any(|l| l.starts_with("gpgsig "));

        Ok(Revision { hash: hash.to_string(), parents, author, timestamp, signed })
    }

    /// Return the deterministic operation set for `hash` (§3.3).
    ///
    /// Root commits (no parents) use `--root`; otherwise diffs against the
    /// first parent. Rename detection is disabled (`--no-renames`) for
    /// byte-level determinism. Raw output lines are
    /// `:<oldmode> <newmode> <oldsha> <newsha> <status>\t<path>`.
    ///
    /// Status letters: `A` → `create`, `D` → `delete`, anything else
    /// (`M`, `T`, mode changes) → `update`. Each operation is a YAML
    /// mapping `{ <verb>: { kind: file, id: <path>, blob: "sha1:<hex>",
    /// prior_blob: "sha1:<hex>" } }` with `blob`/`prior_blob` omitted when
    /// the SHA is the null SHA (all zeros).
    fn operation_set(&self, hash: &str) -> Result<Vec<Value>, String> {
        let sha = hash
            .strip_prefix("sha1:")
            .ok_or_else(|| format!("hash lacks sha1: prefix: {hash}"))?;

        // Determine if this is a root commit.
        let rev = self.revision(hash)?;
        let raw = if rev.parents.is_empty() {
            git(&self.repo, &["diff-tree", "--no-renames", "--root", "-r", "--raw", sha])?
        } else {
            git(&self.repo, &["diff-tree", "--no-renames", "-r", "--raw", sha])?
        };

        let null_sha = "0000000000000000000000000000000000000000";
        let mut ops = Vec::new();

        for line in raw.lines() {
            if !line.starts_with(':') {
                continue;
            }
            // Format: :<oldmode> <newmode> <oldsha> <newsha> <status>\t<path>
            let rest = &line[1..]; // strip leading ':'
            let tab = rest.find('\t').ok_or_else(|| format!("malformed diff-tree line: {line}"))?;
            let meta = &rest[..tab];
            let path = rest[tab + 1..].to_string();

            let mut parts = meta.split_whitespace();
            let _oldmode = parts.next().unwrap_or("");
            let _newmode = parts.next().unwrap_or("");
            let old_sha = parts.next().unwrap_or("").to_string();
            let new_sha = parts.next().unwrap_or("").to_string();
            let status = parts.next().unwrap_or("M").to_string();

            let verb = match status.as_str() {
                "A" => "create",
                "D" => "delete",
                _ => "update",
            };

            let mut inner = Mapping::new();
            inner.insert(Value::String("kind".into()), Value::String("file".into()));
            inner.insert(Value::String("id".into()), Value::String(path.clone()));
            if new_sha != null_sha {
                inner.insert(
                    Value::String("blob".into()),
                    Value::String(format!("sha1:{new_sha}")),
                );
            }
            if old_sha != null_sha {
                inner.insert(
                    Value::String("prior_blob".into()),
                    Value::String(format!("sha1:{old_sha}")),
                );
            }

            let mut op = Mapping::new();
            op.insert(Value::String(verb.into()), Value::Mapping(inner));
            ops.push(Value::Mapping(op));
        }

        Ok(ops)
    }

    /// Return the content-addressed tree hash of the revision as `sha1:<hex>`.
    ///
    /// Uses `git rev-parse <sha>^{tree}`.
    fn state_hash(&self, hash: &str) -> Result<String, String> {
        let sha = hash
            .strip_prefix("sha1:")
            .ok_or_else(|| format!("hash lacks sha1: prefix: {hash}"))?;
        let tree = git(&self.repo, &["rev-parse", &format!("{sha}^{{tree}}")])?;
        Ok(format!("sha1:{}", tree.trim()))
    }

    /// Return all branch heads as `(refname, "sha1:<sha>")` pairs.
    ///
    /// Uses `git for-each-ref refs/heads` with tab-separated output.
    fn heads(&self) -> Result<Vec<(String, String)>, String> {
        let out = git(
            &self.repo,
            &["for-each-ref", "refs/heads", "--format=%(refname)\t%(objectname)"],
        )?;
        let mut heads = Vec::new();
        for line in out.lines() {
            if line.is_empty() {
                continue;
            }
            let mut parts = line.splitn(2, '\t');
            let refname = parts.next().unwrap_or("").to_string();
            let sha = parts.next().unwrap_or("").trim().to_string();
            if !refname.is_empty() && !sha.is_empty() {
                heads.push((refname, format!("sha1:{sha}")));
            }
        }
        Ok(heads)
    }

    /// Verify the authorship signature of a revision.
    ///
    /// Unsigned commits return [`AuthorVerdict::Unsigned`]. Signed commits
    /// are verified via `git verify-commit <sha>`; exit 0 returns
    /// `Verified { principal: <author email>, key_id: String::new() }`.
    /// Git-side key trust configuration (gpg keyrings, ssh allowed-signers)
    /// is deployment configuration; the substrate reports what git verifies.
    fn verify_authorship(&self, hash: &str) -> Result<AuthorVerdict, String> {
        let sha = hash
            .strip_prefix("sha1:")
            .ok_or_else(|| format!("hash lacks sha1: prefix: {hash}"))?;

        let rev = self.revision(hash)?;
        if !rev.signed {
            return Ok(AuthorVerdict::Unsigned);
        }

        // Extract author email for the principal field.
        let author_str = match &rev.author {
            Value::Mapping(m) => {
                m.get(&Value::String("email".into()))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string()
            }
            _ => String::new(),
        };

        // Run git verify-commit; stderr carries the verification output.
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args(["verify-commit", sha])
            .output()
            .map_err(|e| format!("git verify-commit spawn: {e}"))?;

        if out.status.success() {
            Ok(AuthorVerdict::Verified { principal: author_str, key_id: String::new() })
        } else {
            Ok(AuthorVerdict::Failed(
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ))
        }
    }
}

// ── public helpers ────────────────────────────────────────────────────────────

/// Flatten an operation set (as returned by [`Substrate::operation_set`])
/// into `(verb, path)` pairs for use with [`allod_core::policy::GitChange`].
///
/// Each operation is a mapping `{ <verb>: { id: <path>, … } }`. Malformed
/// entries are silently skipped.
pub fn op_paths(ops: &[Value]) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for op in ops {
        if let Value::Mapping(m) = op {
            for (verb_val, inner_val) in m {
                if let (Some(verb), Some(inner)) =
                    (verb_val.as_str(), inner_val.as_mapping())
                {
                    if let Some(path) = inner
                        .get(&Value::String("id".into()))
                        .and_then(Value::as_str)
                    {
                        pairs.push((verb.to_string(), path.to_string()));
                    }
                }
            }
        }
    }
    pairs
}

/// Resolve a commit-ish (branch name, short SHA, tag, etc.) to its full
/// bare SHA string (no prefix) via `git rev-parse --verify <commitish>^{commit}`.
pub fn resolve_commit(repo_dir: &Path, commitish: &str) -> Result<String, String> {
    let raw = git(repo_dir, &["rev-parse", "--verify", &format!("{commitish}^{{commit}}")])?;
    Ok(raw.trim().to_string())
}

/// Read the decision records for `sha` from `refs/notes/allod-decisions`.
///
/// The note body is YAML of the form `{decisions: [...]}`. A missing note
/// returns an empty vec.
///
/// Decision records for git commits live in git notes so the commit itself
/// is never rewritten (§3.4). Decisions attach to the decided SHA;
/// notes travel with `git push origin refs/notes/allod-decisions`.
pub fn read_decisions(repo_dir: &Path, sha: &str) -> Result<Vec<Value>, String> {
    let result = git(repo_dir, &["notes", "--ref=allod-decisions", "show", sha]);
    match result {
        Err(e) if e.contains("found no note") || e.contains("No note found") => {
            return Ok(vec![])
        }
        Err(e) => {
            // Also tolerate "object … is not a commit" or other benign
            // "no note" variants — git's exact message varies by version.
            if e.contains("No note") || e.contains("no note") || e.contains("not found") {
                return Ok(vec![]);
            }
            return Err(e);
        }
        Ok(body) => {
            // Tolerate an empty note body.
            if body.trim().is_empty() {
                return Ok(vec![]);
            }
            let doc: Value =
                serde_yaml::from_str(&body).map_err(|e| format!("note parse: {e}"))?;
            match doc.get("decisions") {
                Some(Value::Sequence(seq)) => Ok(seq.clone()),
                Some(_) => Err("note 'decisions' field is not a sequence".into()),
                None => Ok(vec![]),
            }
        }
    }
}

/// Append a single decision record to the `refs/notes/allod-decisions` note
/// for `sha` (read-modify-write; `git notes --ref=allod-decisions add -f`).
///
/// Decisions accumulate in the `decisions:` list; calling this twice builds
/// a two-element list. The decided commit is never touched (notes attach
/// metadata without rewriting objects — see module doc).
pub fn append_decision(repo_dir: &Path, sha: &str, record: &Value) -> Result<(), String> {
    // Read existing decisions.
    let mut existing = read_decisions(repo_dir, sha)?;
    existing.push(record.clone());

    // Serialize as { decisions: [...] }.
    let mut root = Mapping::new();
    root.insert(
        Value::String("decisions".into()),
        Value::Sequence(existing),
    );
    let body = serde_yaml::to_string(&Value::Mapping(root))
        .map_err(|e| format!("serialize decisions: {e}"))?;

    // Write the new note via a temp file to avoid stdin complexity and shell
    // escaping. The temp file path uses the sha to avoid collisions.
    let tmp = std::env::temp_dir().join(format!("allod-note-{sha}.yaml"));
    std::fs::write(&tmp, &body).map_err(|e| format!("write temp note: {e}"))?;
    let tmp_str = tmp.to_string_lossy().to_string();
    let result =
        git(repo_dir, &["notes", "--ref=allod-decisions", "add", "-f", "-F", &tmp_str, sha]);
    let _ = std::fs::remove_file(&tmp);
    result?;

    Ok(())
}

// ── private helpers ───────────────────────────────────────────────────────────

/// Parse `"Name <email>"` into `(name, email)`.
fn parse_author(s: &str) -> (String, String) {
    if let (Some(lt), Some(gt)) = (s.rfind('<'), s.rfind('>')) {
        let name = s[..lt].trim().to_string();
        let email = s[lt + 1..gt].trim().to_string();
        (name, email)
    } else {
        (s.to_string(), String::new())
    }
}
