//! Adopting a route registry somebody else wrote.
//!
//! An operator hands Brama a registry document and asks what it would mean
//! here. This file names the four subjects that answer come from and holds what
//! all of them share: the bounds an offered document is held to before anything
//! reads it, and the words a candidate alias is reported with.
//!
//! `document` owns the exact shape a document may have and refuses everything
//! else by name. `identity` owns who asked and under which agent. `preview`
//! owns the answer that touches nothing: what each alias would become, which
//! deployments it needs, and which identities exist for it. `apply` owns the one
//! atomic write and the counts it reports.
//!
//! Reading and writing stay apart on purpose: a review can be run against a
//! serving gateway, and only `apply` replaces the destination.

mod apply;
mod document;
mod identity;
mod preview;

use serde::Serialize;

use crate::core::inference_routes;

pub use apply::{apply_document, AdoptionItemResult, AdoptionResult};
pub use preview::{
    preview_document, AdoptionCandidate, AdoptionPreview, AdoptionProviderIdentity,
    AdoptionSubscriptionIdentity,
};

/// The bounds every offered document is held to before a single alias is read.
///
/// They are here rather than in `document` because the review, the write and
/// the shape all answer to the same limits, and one copy of a limit is one
/// answer about it.
const MAX_DOCUMENT_BYTES: usize = 1024 * 1024;
const MAX_SOURCE_NAME_CHARACTERS: usize = 512;
const MAX_AGENT_ID_CHARACTERS: usize = 128;
const MAX_ALIASES: usize = 1024;
const MAX_DEPLOYMENTS: usize = 128;
const MAX_DESTINATION_CHARACTERS: usize = 512;

/// The registry schema this build reads and writes.
const SCHEMA_VERSION: u32 = 1;

/// What adoption decided about one alias.
///
/// `Importable` is a review verdict and the other four are outcomes, which is
/// why the review and the write report the same enum: an operator comparing the
/// two reads one vocabulary, not two.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AdoptionDisposition {
    Importable,
    Imported,
    Unchanged,
    Conflicting,
    Rejected,
}

/// Where an accepted adoption is written when the operator names no
/// destination.
///
/// The configured registry path wins, because a deployment that declares one is
/// declaring where its routes live; the home-relative path is the answer only
/// when nothing declares it.
pub fn default_destination() -> Result<std::path::PathBuf, String> {
    if let Some(path) = inference_routes::configured_path() {
        return Ok(path);
    }
    let home = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            "HOME is required when BRAMA_INFERENCE_ROUTES_FILE is not set".to_string()
        })?;
    Ok(std::path::PathBuf::from(home)
        .join(".config")
        .join("brama")
        .join("inference-routes.json"))
}
