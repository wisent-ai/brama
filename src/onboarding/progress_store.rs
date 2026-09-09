//! Where recorded first-use progress lives on this machine.

use std::path::PathBuf;

pub(super) fn state_path() -> PathBuf {
    if let Some(path) = std::env::var_os("XDG_STATE_HOME") {
        return PathBuf::from(path).join("brama/onboarding.json");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".local/state/brama/onboarding.json");
    }
    std::env::temp_dir().join("brama/onboarding.json")
}
