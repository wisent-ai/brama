//! `brama routes set|rm|migrate` against this host's route registry file,
//! through the same validated atomic writes the admin routes use.

use std::path::PathBuf;

use serde_json::Value;

/// The registry file this command acts on.
fn registry_path(file: Option<PathBuf>) -> PathBuf {
    match file {
        Some(path) => path,
        None => match brama::config_adoption::default_destination() {
            Ok(path) => path,
            Err(error) => {
                eprintln!("route registry error: {error}");
                std::process::exit(1);
            }
        },
    }
}

/// Declare one alias, through the same validated atomic write
/// `PUT /v1/admin/routes` uses.
///
/// The shape is checked here — an alias that promises a typed answer cannot
/// be pointed at a route that cannot produce one — and whether the route can
/// be served on this host is what `brama aliases` reports afterwards. An
/// operator shell holds no provider capability, so refusing the write for a
/// credential this process cannot see would make the registry unwritable
/// from the one place a gateway that is not running can be repaired.
pub(super) fn set(alias: String, destination: String, file: Option<PathBuf>, json: bool) {
    if !brama::core::server::valid_alias(&alias) {
        eprintln!(
            "route alias `{alias}` is invalid: an alias is lowercase letters, digits, `-`, `_`, `.` and `/`"
        );
        std::process::exit(1);
    }
    if !brama::core::server::route_shape_writable(&alias, &destination) {
        eprintln!(
            "alias `{alias}` cannot carry `{destination}`: it is not a shape this alias promises"
        );
        std::process::exit(1);
    }
    let path = registry_path(file);
    let document = match brama::core::inference_routes::update_route(&path, &alias, &destination) {
        Ok(document) => document,
        Err(error) => {
            eprintln!("route update error: {error}");
            std::process::exit(1);
        }
    };
    report_registry(&path, &document, json, &format!("{alias} -> {destination}"));
}

pub(super) fn remove(alias: String, file: Option<PathBuf>, json: bool) {
    if !brama::core::server::valid_alias(&alias) {
        eprintln!("route alias `{alias}` is invalid");
        std::process::exit(1);
    }
    let path = registry_path(file);
    let document = match brama::core::inference_routes::delete_route(&path, &alias) {
        Ok(document) => document,
        Err(error) => {
            eprintln!("route removal error: {error}");
            std::process::exit(1);
        }
    };
    report_registry(&path, &document, json, &format!("{alias} removed"));
}

fn report_registry(path: &std::path::Path, document: &Value, json: bool, change: &str) {
    if json {
        super::super::print_json(&serde_json::json!({
            "registry": path.display().to_string(),
            "change": change,
            "document": document,
        }));
        return;
    }
    println!("registry: {}", path.display());
    println!("change: {change}");
    let routes = document.get("routes").and_then(Value::as_object);
    println!("routes: {}", routes.map_or(0, serde_json::Map::len));
    for (alias, destination) in routes.into_iter().flatten() {
        println!("{:<32} {}", alias, destination_text(destination));
    }
}

/// A route's destination: its text, or the JSON it is written as when it is
/// not text, so a destination of another shape is shown rather than a dash.
fn destination_text(destination: &Value) -> String {
    match destination.as_str() {
        Some(text) => text.to_string(),
        None => destination.to_string(),
    }
}

/// Move one operator's registry onto the current document shape, in place and
/// through the same validated atomic write every other route change uses.
pub(super) fn migrate(file: Option<PathBuf>, json: bool) {
    let path = registry_path(file);
    let document = match brama::core::inference_routes::migrate(&path) {
        Ok(document) => document,
        Err(error) => {
            eprintln!("route migration error: {error}");
            std::process::exit(1);
        }
    };
    if json {
        super::super::print_json(&serde_json::json!({
            "registry": path.display().to_string(),
            "document": document,
        }));
        return;
    }
    println!("registry: {}", path.display());
    println!(
        "deployments: {}",
        document
            .get("deployments")
            .and_then(Value::as_array)
            .map_or(0, Vec::len)
    );
    let routes = document.get("routes").and_then(Value::as_object);
    println!("routes: {}", routes.map_or(0, serde_json::Map::len));
    for (alias, destination) in routes.into_iter().flatten() {
        println!("{:<32} {}", alias, destination_text(destination));
    }
}
