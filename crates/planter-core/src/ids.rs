use serde::{Deserialize, Serialize};

/// Correlates a response to a request in IPC streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ReqId(
    /// Monotonic numeric request identifier.
    pub u64,
);

/// Identifies an isolated execution cell.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CellId(
    /// Opaque cell identifier string.
    pub String,
);

/// Identifies a launched job.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JobId(
    /// Opaque job identifier string.
    pub String,
);

/// Identifies an interactive PTY session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(
    /// Monotonic numeric PTY session identifier.
    pub u64,
);
