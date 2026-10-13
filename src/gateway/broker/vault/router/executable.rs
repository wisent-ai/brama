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

/// The vault binary this gateway shells for every credential operation.
///
/// Four places, in this order: the router's own declaration, the vault's
/// declaration, an executable of either name on `PATH`, and the path the
/// fleet installs it at. The bare word `entitlements-router` used to be the
/// whole answer, and on a machine where the vault is installed as
/// `~/.stado/bin/skarbiec` and nothing exports either variable, every
/// credential write fails with `No such file or directory (os error 2)` and
/// names no program — `brama subscription sync` answers that for every
/// grant the machine holds, while nothing that needs a working subscription
/// can be served.
pub(crate) fn entitlements_router_bin() -> Result<String, String> {
    for declaration in [ENTITLEMENTS_ROUTER_BIN_ENV, SKARBIEC_BIN_ENV] {
        match std::env::var(declaration) {
            Ok(value) if value.trim().is_empty() => {
                return Err(format!("resolve vault executable: {declaration} is empty"));
            }
            Ok(value) => return Ok(value.trim().to_owned()),
            Err(std::env::VarError::NotPresent) => {}
            Err(error) => {
                return Err(format!(
                    "read vault executable declaration {declaration}: {error}"
                ))
            }
        }
    }
    if let Some(found) = VAULT_PROGRAM_NAMES.iter().copied().find_map(on_path) {
        return Ok(found);
    }
    if let Some(home) = std::env::var_os("HOME") {
        let installed = std::path::Path::new(&home).join(VAULT_HOME_RELATIVE);
        if is_executable_file(&installed) {
            return Ok(installed.display().to_string());
        }
    }
    Ok(DEFAULT_ENTITLEMENTS_ROUTER_BIN.to_owned())
}

/// Is `path` a file this process could execute?
fn is_executable_file(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|info| info.is_file() && info.permissions().mode() & 0o111 != 0)
}

/// The first executable named `program` on `PATH`.
fn on_path(program: &str) -> Option<String> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|directory| directory.join(program))
        .find(|candidate| is_executable_file(candidate))
        .map(|candidate| candidate.display().to_string())
}
