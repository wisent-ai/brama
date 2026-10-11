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
    let named = std::env::var("SKARBIEC_CAPABILITY_ROUTES_FILE")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            settings
                .get("SKARBIEC_CAPABILITY_ROUTES_FILE")
                .cloned()
                .filter(|value| !value.is_empty())
        });
    // On a running host the routes table is not beside the vault the broker
    // opens; looking only there grants nothing and leaves every redemption
    // denied, silently.
    let mut candidates: Vec<PathBuf> = named.map(PathBuf::from).into_iter().collect();
    candidates.push(Path::new(vault).with_file_name("capability-routes.json"));
    candidates.push(home.join(".stado/capability-routes.json"));
    let Some(routes_path) = candidates.iter().find(|path| path.is_file()) else {
        let named: Vec<String> = candidates
            .iter()
            .map(|path| path.display().to_string())
            .collect();
        return Err(format!(
            "no capability routes table found at {}",
            named.join(", ")
        ));
    };
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
