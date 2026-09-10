//! The operator's route registry: which alias points at which single
//! destination, and which local deployments exist to be pointed at.
//!
//! One alias resolves to exactly one destination. When that destination cannot
//! answer, the caller is told so; the registry holds no second address for the
//! same alias, because a request that quietly went somewhere else than the
//! operator named is a request nobody can account for afterwards.
//!
//! The family is split by what a reader is looking for: [`document`] owns the
//! document shape, the refusals a registry file must survive and the single
//! atomic write; [`resolution`] owns turning a written destination into the one
//! a request is sent to; [`editing`] owns operator edits to an existing file,
//! including the migration onto the current document shape; [`importing`] owns
//! merging a reviewed configuration into the file.

mod document;
mod editing;
mod importing;
mod resolution;

use std::path::PathBuf;

pub use document::{snapshot, validate_document};
pub use editing::{delete_route, migrate, update_route};
pub use importing::{import_routes, RouteImport, RouteImportDisposition, RouteImportResult};
pub use resolution::{base_url, resolve, resolved, validate};

pub const ROUTES_FILE_ENV: &str = "BRAMA_INFERENCE_ROUTES_FILE";

pub fn configured_path() -> Option<PathBuf> {
    std::env::var_os(ROUTES_FILE_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}
