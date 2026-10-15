//! Which vault executable this gateway shells, and where it is looked for.

pub(super) const ENTITLEMENTS_ROUTER_BIN_ENV: &str = "ENTITLEMENTS_ROUTER_BIN";
/// The vault binary's second declaration, the one Skarbiec's own fixtures and
/// Stado's release journeys set.
pub(super) const SKARBIEC_BIN_ENV: &str = "SKARBIEC_BIN";
const DEFAULT_ENTITLEMENTS_ROUTER_BIN: &str = "entitlements-router";
/// The installed vault CLI, in the two names it is reached by on this fleet.
const VAULT_PROGRAM_NAMES: &[&str] = &[DEFAULT_ENTITLEMENTS_ROUTER_BIN, "skarbiec"];
/// Where the fleet installs it when nothing is on `PATH`, relative to `$HOME`.
const VAULT_HOME_RELATIVE: &str = ".stado/bin/skarbiec";

/// Whether a file mode carries any execute bit, as the C library names them.
fn executable_mode(mode: u32) -> bool {
    let execute = u32::from(libc::S_IXUSR | libc::S_IXGRP | libc::S_IXOTH);
    std::num::NonZeroU32::new(mode & execute).is_some()
}

/// The vault binary this gateway shells for every credential operation.
///
/// Four places, in this order: the router's own declaration, the vault's
/// declaration, an executable of either name on `PATH`, and the path the
/// fleet installs it at. When none holds one the answer is the refusal
/// naming all four: shelling the bare word instead failed every credential
/// write with `No such file or directory (os error 2)` and named no program.
pub(crate) fn entitlements_router_bin() -> Result<String, String> {
    for declaration in [ENTITLEMENTS_ROUTER_BIN_ENV, SKARBIEC_BIN_ENV] {
        let declared = crate::environment::declared_text(declaration)
            .map_err(|refusal| format!("read vault executable declaration: {refusal}"))?;
        match declared {
            Some(value) if value.trim().is_empty() => {
                return Err(format!("resolve vault executable: {declaration} is empty"));
            }
            Some(value) => return Ok(value.trim().to_owned()),
            None => {}
        }
    }
    if let Some(found) = VAULT_PROGRAM_NAMES.iter().copied().find_map(on_path) {
        return Ok(found);
    }
    let installed = match std::env::var_os("HOME").filter(|home| !home.is_empty()) {
        Some(home) => {
            use std::os::unix::fs::PermissionsExt;
            let installed = std::path::Path::new(&home).join(VAULT_HOME_RELATIVE);
            match std::fs::metadata(&installed) {
                Ok(info) if info.is_file() && executable_mode(info.permissions().mode()) => {
                    return Ok(installed.display().to_string());
                }
                Ok(_) => format!("{} is not an executable file", installed.display()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    format!("{} does not exist", installed.display())
                }
                Err(error) => format!("{} cannot be examined: {error}", installed.display()),
            }
        }
        None => format!("HOME is not set, so ~/{VAULT_HOME_RELATIVE} cannot be looked at"),
    };
    Err(format!(
        "resolve vault executable: neither {ENTITLEMENTS_ROUTER_BIN_ENV} nor {SKARBIEC_BIN_ENV} \
         is set, no {} is an executable on PATH, and {installed}",
        VAULT_PROGRAM_NAMES.join(" or ")
    ))
}

/// Is `path` a file this process could execute?
fn is_executable_file(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|info| info.is_file() && executable_mode(info.permissions().mode()))
}

/// The first executable named `program` on `PATH`.
fn on_path(program: &str) -> Option<String> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|directory| directory.join(program))
        .find(|candidate| is_executable_file(candidate))
        .map(|candidate| candidate.display().to_string())
}
