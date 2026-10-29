//! The owner-only file that keeps a standalone process's subscription grants
//! across restarts: `BRAMA_STANDALONE_GRANTS_FILE` when the launcher names
//! one, otherwise `.config/brama/standalone-grants.json` under the Brama home,
//! where the usage ledger lives too.
//!
//! It holds one JSON object from each subscription resource to its grant
//! document, the same map the process keeps in memory. It is read once, when
//! the provider map is installed, and written whole, staged beside itself and
//! renamed over, after every donation, renewal and removal, so the file is
//! always one complete map and a refresh token the provider rotated survives
//! the next restart. A file another user can read is refused, as the local
//! credentials file is, because it holds live grants.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use zeroize::{Zeroize, Zeroizing};

use super::LocalCredentialMap;

const GRANTS_FILE_ENV: &str = "BRAMA_STANDALONE_GRANTS_FILE";

/// The grants file: the launcher's declaration, or its place in the Brama home.
fn path() -> Result<PathBuf, String> {
    if let Some(declared) = crate::environment::declared_setting(GRANTS_FILE_ENV)? {
        return Ok(PathBuf::from(declared));
    }
    let home = crate::journal::home().map_err(|cause| {
        format!(
            "cannot resolve the standalone grants file: {GRANTS_FILE_ENV} is not set and {cause}"
        )
    })?;
    Ok(home
        .join(".config")
        .join("brama")
        .join("standalone-grants.json"))
}

/// Refuse a grants file any other user can reach.
fn check_private(path: &Path) -> Result<(), String> {
    let mode = std::fs::metadata(path)
        .map_err(|error| {
            format!(
                "{GRANTS_FILE_ENV} {} cannot be examined: {error}",
                path.display()
            )
        })?
        .permissions()
        .mode();
    let others = crate::core::mode::GROUP_AND_OTHER;
    // Any group or other permission bit set is a reader besides the owner.
    if mode & !others != mode {
        return Err(format!(
            "{GRANTS_FILE_ENV} {} must be readable by its owner only (mode {mode:o})",
            path.display()
        ));
    }
    Ok(())
}

/// Read the grants file into `held`. A file that does not exist yet holds no
/// grant; every other failure to read it refuses the install.
pub(super) fn load(held: &mut LocalCredentialMap) -> Result<(), String> {
    let path = path()?;
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => Zeroizing::new(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "{GRANTS_FILE_ENV} {} cannot be read: {error}",
                path.display()
            ))
        }
    };
    check_private(&path)?;
    let grants: HashMap<String, String> = serde_json::from_str(&text).map_err(|error| {
        format!(
            "{GRANTS_FILE_ENV} {} is not a map from subscription to grant: {error}",
            path.display()
        )
    })?;
    for (resource, mut grant) in grants {
        held.insert(resource, Zeroizing::new(grant.as_bytes().to_vec()));
        grant.zeroize();
    }
    Ok(())
}

/// Write `held` whole to the grants file, creating its directory when absent.
pub(super) fn save(held: &LocalCredentialMap) -> Result<(), String> {
    let path = path()?;
    let mut grants: HashMap<&str, &str> = HashMap::with_capacity(held.len());
    for (resource, grant) in held {
        let grant = std::str::from_utf8(grant).map_err(|error| {
            format!(
                "the grant of {resource} is not text, so {GRANTS_FILE_ENV} cannot hold it: {error}"
            )
        })?;
        grants.insert(resource, grant);
    }
    let encoded = Zeroizing::new(
        serde_json::to_vec(&grants)
            .map_err(|error| format!("the grants cannot be encoded: {error}"))?,
    );
    let parent = path
        .parent()
        .ok_or_else(|| format!("{GRANTS_FILE_ENV} {} names no directory", path.display()))?;
    std::fs::create_dir_all(parent).map_err(|error| {
        format!(
            "cannot create the grants directory {}: {error}",
            parent.display()
        )
    })?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("{GRANTS_FILE_ENV} {} names no file", path.display()))?;
    // One name per save: a file a crashed save left under a name tied to the
    // process id would refuse every later save of a process given that id
    // (`create_new`) with its existence instead of a real cause.
    let staging = parent.join(format!(".{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
    let written = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(crate::core::mode::OWNER_READ_WRITE)
            .open(&staging)
            .map_err(|error| format!("cannot create {}: {error}", staging.display()))?;
        file.write_all(&encoded)
            .map_err(|error| format!("cannot write {}: {error}", staging.display()))?;
        file.sync_all()
            .map_err(|error| format!("cannot sync {}: {error}", staging.display()))?;
        std::fs::set_permissions(
            &staging,
            std::fs::Permissions::from_mode(crate::core::mode::OWNER_READ_WRITE),
        )
        .map_err(|error| {
            format!(
                "cannot restrict {} to its owner: {error}",
                staging.display()
            )
        })?;
        std::fs::rename(&staging, &path).map_err(|error| {
            format!(
                "cannot replace {} with the new grants: {error}",
                path.display()
            )
        })
    })();
    written.map_err(|error| crate::core::staging::failed_with(error, &staging))
}
