use std::time::{SystemTime, UNIX_EPOCH};

/// Returns the current UNIX time in milliseconds.
pub fn now_ms() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_millis().try_into().unwrap_or(u64::MAX),
        Err(_) => 0,
    }
}
