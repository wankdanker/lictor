use lictor_store::StoreError;

/// Errors returned by [`crate::Registry`] operations.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum RegistryError {
    /// The request is malformed or violates a constraint.
    #[error("invalid request: {0}")]
    Invalid(String),
    /// The named thing does not exist.
    #[error("{0} not found")]
    NotFound(String),
    /// A conflicting write won.
    #[error("conflict: {0}")]
    Conflict(String),
    /// A newer run of this node took over; this agent must stop.
    #[error("node '{0}' was taken over by a newer agent generation; this agent is fenced")]
    Fenced(String),
    /// The operation needs something this implementation does not offer.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// The catalog has not finished its initial load.
    #[error("not ready: {0}")]
    NotReady(String),
    /// The backing store failed.
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl RegistryError {
    /// A short, stable code for API responses: `invalid`, `not_found`, `conflict`, ...
    pub fn code(&self) -> &'static str {
        match self {
            RegistryError::Invalid(_) => "invalid",
            RegistryError::NotFound(_) => "not_found",
            RegistryError::Conflict(_) => "conflict",
            RegistryError::Fenced(_) => "fenced",
            RegistryError::Unsupported(_) => "unsupported",
            RegistryError::NotReady(_) => "not_ready",
            RegistryError::Store(StoreError::Unavailable(_)) => "unavailable",
            RegistryError::Store(StoreError::Conflict { .. }) => "conflict",
            RegistryError::Store(_) => "store",
        }
    }
}

/// Result alias for registry operations.
pub type Result<T, E = RegistryError> = std::result::Result<T, E>;
