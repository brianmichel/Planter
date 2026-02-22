use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Stable error classes exchanged over the planter protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The request was malformed or semantically invalid.
    InvalidRequest,
    /// The referenced resource does not exist.
    NotFound,
    /// The requested operation exceeded its deadline.
    Timeout,
    /// Client and daemon protocol versions are incompatible.
    ProtocolMismatch,
    /// The service is temporarily unavailable.
    Unavailable,
    /// An unexpected internal failure occurred.
    Internal,
}

/// Structured error payload returned by daemon and worker operations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
#[error("{code:?}: {message}")]
pub struct PlanterError {
    /// High-level error category.
    pub code: ErrorCode,
    /// Human-readable summary.
    pub message: String,
    /// Optional extended context for debugging.
    pub detail: Option<String>,
}
