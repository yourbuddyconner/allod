#[derive(Debug, thiserror::Error)]
pub enum AllodError {
    #[error("schema violation: {0}")]
    SchemaViolation(String),
    #[error("policy rejected: {0}")]
    PolicyRejected(String),
    #[error("hash mismatch: {0}")]
    HashMismatch(String),
    #[error("invalid signature: {0}")]
    SignatureInvalid(String),
    #[error("unknown principal: {0}")]
    UnknownPrincipal(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("storage: {0}")]
    Storage(String),
    #[error("{0}")]
    Other(String),
}

impl From<String> for AllodError {
    fn from(s: String) -> Self {
        AllodError::Other(s)
    }
}
