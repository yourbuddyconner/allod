//! Embedded reference ontologies: compile-time include of the repo's ontology YAML files.
//!
//! `embedded_profile` mirrors exactly what `flows::profile_from_dir` produces for the
//! same profile name, but sources files via `include_str!` rather than `std::fs` —
//! making it available in WASM and other no-filesystem targets.

use crate::AllodError;
use crate::flows::ProfileSource;

// Embedded ontology files (paths are relative to this source file, i.e. from
// crates/allod-graph/src/ up three levels to the repo root, then into ontologies/).
const CORE_ONTOLOGY: &str = include_str!("../../../ontologies/core/ontology.yaml");
const MEMORY_ONTOLOGY: &str = include_str!("../../../ontologies/memory/ontology.yaml");
const MEMORY_TAXONOMY: &str = include_str!("../../../ontologies/memory/taxonomy.yaml");
const MEMORY_POLICY_LOCAL: &str = include_str!("../../../ontologies/memory/policy-local.yaml");
const CODE_ONTOLOGY: &str = include_str!("../../../ontologies/code/ontology.yaml");
const ENG_TAXONOMY: &str = include_str!("../../../ontologies/eng/taxonomy.yaml");
const CODE_POLICY_LOCAL: &str = include_str!("../../../ontologies/code/policy-local.yaml");

/// Return a `ProfileSource` whose documents are byte-identical to what
/// `flows::profile_from_dir` would produce for the same profile name.
///
/// Supported names: `"memory"`, `"code"`.
pub fn embedded_profile(name: &str) -> Result<ProfileSource, AllodError> {
    let parse = |text: &str| -> Result<serde_yaml::Value, AllodError> {
        serde_yaml::from_str(text).map_err(|e| AllodError::Other(e.to_string()))
    };

    match name {
        "memory" => Ok(ProfileSource {
            name: "memory".to_string(),
            docs: vec![
                ("core".to_string(), parse(CORE_ONTOLOGY)?),
                ("memory".to_string(), parse(MEMORY_ONTOLOGY)?),
                ("memory-taxonomy".to_string(), parse(MEMORY_TAXONOMY)?),
            ],
            policy: parse(MEMORY_POLICY_LOCAL)?,
        }),
        "code" => Ok(ProfileSource {
            name: "code".to_string(),
            docs: vec![
                ("core".to_string(), parse(CORE_ONTOLOGY)?),
                ("code".to_string(), parse(CODE_ONTOLOGY)?),
                ("eng-taxonomy".to_string(), parse(ENG_TAXONOMY)?),
            ],
            policy: parse(CODE_POLICY_LOCAL)?,
        }),
        other => Err(AllodError::Other(format!("unknown profile {other:?}"))),
    }
}
