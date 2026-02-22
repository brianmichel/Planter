use std::{env, path::PathBuf};

/// Resolves the daemon state directory from environment and platform defaults.
pub fn default_state_dir() -> PathBuf {
    if let Some(override_dir) = env::var_os("PLANTER_STATE_DIR") {
        return PathBuf::from(override_dir);
    }

    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home).join(".planter/state");
    }

    PathBuf::from(".planter/state")
}
