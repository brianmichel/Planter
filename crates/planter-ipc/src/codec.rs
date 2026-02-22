use serde::{Serialize, de::DeserializeOwned};

use crate::IpcError;

/// Serializes a value to CBOR bytes for wire transmission.
pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, IpcError> {
    serde_cbor::to_vec(value).map_err(|err| IpcError::Encode(err.to_string()))
}

/// Deserializes a CBOR frame payload into a typed value.
pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, IpcError> {
    serde_cbor::from_slice(bytes).map_err(|err| IpcError::Decode(err.to_string()))
}
