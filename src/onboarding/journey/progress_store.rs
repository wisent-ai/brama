//! Where recorded first-use progress lives on this machine: under
//! `XDG_STATE_HOME` when it names a directory, else under the user's home in
//! `.local/state`, the XDG default. The home is the one Brama keeps all its
//! state under (`crate::journal::home`), and without one the journey is
//! refused with its cause; progress is never written to the system's
//! temporary directory, where it would vanish and be shared.

use std::path::PathBuf;

pub(crate) fn state_path() -> Result<PathBuf, String> {
    match std::env::var_os("XDG_STATE_HOME").filter(|path| !path.is_empty()) {
        Some(path) => Ok(PathBuf::from(path).join("brama/onboarding.json")),
        None => Ok(crate::journal::home()?.join(".local/state/brama/onboarding.json")),
    }
}
