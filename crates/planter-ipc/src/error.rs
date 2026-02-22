use thiserror::Error;

/// Transport and serialization failures for planter IPC operations.
#[derive(Debug, Error)]
pub enum IpcError {
    /// Underlying socket I/O error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// Request/response encoding failed.
    #[error("failed to encode cbor payload: {0}")]
    Encode(String),
    /// Request/response decoding failed.
    #[error("failed to decode cbor payload: {0}")]
    Decode(String),
    /// Operation exceeded configured timeout.
    #[error("request timed out")]
    Timeout,
    /// Frame size exceeded maximum allowed payload.
    #[error("frame too large: {size} > {max}")]
    FrameTooLarge { size: u32, max: u32 },
    /// Response did not match request identifier.
    #[error("request id mismatch: expected {expected}, got {actual}")]
    RequestIdMismatch { expected: u64, actual: u64 },
    /// Peer protocol version did not match local expectation.
    #[error("protocol mismatch: expected {expected}, got {actual}")]
    ProtocolMismatch { expected: u32, actual: u32 },
}
