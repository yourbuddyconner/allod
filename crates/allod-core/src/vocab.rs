//! The specification's controlled vocabulary, kept sorted for stable
//! diagnostic output.

/// Attribute base types (§1.3).
pub const BASE_TYPES: &[&str] = &[
    "string", "int", "float", "decimal", "bool", "timestamp", "date",
    "duration", "bytes", "node-ref", "edge-ref", "document-ref",
    "external-ref",
];

/// Edge cardinalities (§2.1).
pub const CARDINALITIES: &[&str] =
    &["many-to-many", "many-to-one", "one-to-many", "one-to-one"];

/// Default postures (§4.1).
pub const POSTURES: &[&str] = &["open", "restricted"];

/// Leaf selector keys (§4.1).
pub const SELECTOR_KEYS: &[&str] = &[
    "author_kind", "basis", "operation", "region", "repo", "substrate",
    "target_ref", "type",
];

/// Principal kinds (§6.1).
pub const AUTHOR_KINDS: &[&str] = &["agent", "service", "user"];

/// Classification and lineage bases (§1.6, §5.1).
pub const BASES: &[&str] = &["deterministic", "manual", "model-assisted"];

/// Rule requirement vocabulary (§4.2).
pub const REQUIREMENT_KEYS: &[&str] = &[
    "attestation_required", "authors", "classification_required",
    "reviewers", "review_window", "schema_valid", "substrate_checks",
];

/// Operation verbs (§3.2.2).
pub const OPERATIONS: &[&str] = &[
    "create", "update", "delete", "resolve", "redact-document",
    "redact-operation", "define-type", "set-policy", "deprecate-term",
];

/// Instance document kinds (§1.1, §3.2.1, §4.3).
pub const DOC_KINDS: &[&str] = &[
    "changeset", "classification", "decision-record", "document", "edge",
    "node",
];

/// Decision record verdicts (§4.3).
pub const VERDICTS: &[&str] = &["abstain", "approve", "reject"];

/// Document storage modes (§1.5).
pub const STORAGE: &[&str] = &["external", "inline", "stored"];

/// Taxonomy term statuses (§2.2).
pub const TERM_STATUS: &[&str] = &["active", "deprecated"];

/// Fields whose values are hashes and need algorithm prefixes (§1.7).
pub const HASH_FIELDS: &[&str] = &[
    "content_hash", "hash", "policy_context", "rev", "schema_context",
    "state_hash",
];
