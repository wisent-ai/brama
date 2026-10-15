//! The capability routes table `register` grants from: where it is looked
//! for, and the vault coordinates it maps.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::Value;

/// The routes table this installation names and every `acquire:<item>#<field>`
/// capability it maps, sorted. A table that cannot be found, read or parsed,
/// or that maps nothing, is refused by name: granting nothing would leave
/// every redemption denied without a word.
pub(super) fn capabilities(
    home: &Path,
    vault: &str,
    settings: &BTreeMap<String, String>,
) -> Result<(PathBuf, Vec<String>), String> {
    // An environment value that is not UTF-8 is a broken naming of the table,
    // not an absent one: falling through to the settings would grant from a
    // table nobody asked for.
    let from_env = brama::environment::declared_setting("SKARBIEC_CAPABILITY_ROUTES_FILE")?;
    let named = from_env.or_else(|| {
        settings
            .get("SKARBIEC_CAPABILITY_ROUTES_FILE")
            .cloned()
            .filter(|value| !value.is_empty())
    });
    // On a running host the routes table is not beside the vault the broker
    // opens; looking only there grants nothing and leaves every redemption
    // denied, silently. A table named explicitly is that table or a refusal;
    // of the default places, only one that does not exist is passed over, so
    // an unreadable table is not replaced by the next one found.
    let present = |path: &Path| match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(true),
        Ok(_) => Err(format!("{} is not a file", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!(
            "cannot examine capability routes table {}: {error}",
            path.display()
        )),
    };
    let routes_path = match named.map(PathBuf::from) {
        Some(named) => {
            if !present(&named)? {
                return Err(format!(
                    "SKARBIEC_CAPABILITY_ROUTES_FILE names {}, which does not exist",
                    named.display()
                ));
            }
            named
        }
        None => {
            let candidates = [
                Path::new(vault).with_file_name("capability-routes.json"),
                home.join(".stado/capability-routes.json"),
            ];
            let mut found = None;
            for candidate in &candidates {
                if present(candidate)? {
                    found = Some(candidate.clone());
                    break;
                }
            }
            match found {
                Some(path) => path,
                None => {
                    let looked: Vec<String> = candidates
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect();
                    return Err(format!(
                        "no capability routes table found at {}",
                        looked.join(", ")
                    ));
                }
            }
        }
    };
    let routes_path = &routes_path;
    let bytes = std::fs::read(routes_path).map_err(|error| {
        format!(
            "cannot read capability routes table {}: {error}",
            routes_path.display()
        )
    })?;
    let routes: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("{} is not JSON: {error}", routes_path.display()))?;
    let capabilities: BTreeSet<String> = routes
        .as_object()
        .into_iter()
        .flat_map(|entries| entries.values())
        .filter_map(|entry| {
            let item = entry
                .get("item")?
                .as_str()
                .filter(|value| !value.is_empty())?;
            let field = entry
                .get("field")?
                .as_str()
                .filter(|value| !value.is_empty())?;
            Some(format!("acquire:{item}#{field}"))
        })
        .collect();
    if capabilities.is_empty() {
        return Err(format!(
            "{} maps nothing, so there is nothing to grant",
            routes_path.display()
        ));
    }
    Ok((routes_path.clone(), capabilities.into_iter().collect()))
}
