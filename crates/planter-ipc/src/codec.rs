use serde::{Serialize, de::DeserializeOwned};

use crate::IpcError;

pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, IpcError> {
    serde_cbor::to_vec(value).map_err(|err| IpcError::Encode(err.to_string()))
}

pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, IpcError> {
    serde_cbor::from_slice(bytes).map_err(|err| IpcError::Decode(err.to_string()))
}
