use thiserror::Error;

#[derive(Debug, Error)]
pub enum IpcError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to encode cbor payload: {0}")]
    Encode(String),
    #[error("failed to decode cbor payload: {0}")]
    Decode(String),
    #[error("request timed out")]
    Timeout,
    #[error("frame too large: {size} > {max}")]
    FrameTooLarge { size: u32, max: u32 },
    #[error("request id mismatch: expected {expected}, got {actual}")]
    RequestIdMismatch { expected: u64, actual: u64 },
    #[error("protocol mismatch: expected {expected}, got {actual}")]
    ProtocolMismatch { expected: u32, actual: u32 },
}
