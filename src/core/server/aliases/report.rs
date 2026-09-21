//! Every declared alias with its state, for a reader that is not the running
//! gateway: the CLI and any console holding no bearer for it.

use std::path::PathBuf;

use serde::Serialize;

use super::diagnosis::AliasDiagnosis;
use super::table::ModelAliases;
use super::MODEL_ALIASES_ENV;

/// Where an alias report was read from, so the reader can tell a gateway's
/// answer from an operator shell's answer.
#[derive(Clone, Debug, Serialize)]
pub struct AliasReportSource {
    /// The launcher's alias table, when this process was started with it.
    pub launcher_table_present: bool,
    /// The route registry file that was read, if any.
    pub routes_file: Option<PathBuf>,
}

/// Every declared alias with its state, plus the sources the answer came from.
#[derive(Clone, Debug, Serialize)]
pub struct AliasReport {
    pub source: AliasReportSource,
    pub aliases: Vec<AliasDiagnosis>,
}

impl AliasReport {
    pub fn unserviceable(&self) -> usize {
        self.aliases.iter().filter(|alias| !alias.serving()).count()
    }
}

/// Every declared alias with its state, for the CLI and any console that has
/// no bearer for the running gateway.
///
/// Read from the same sources the server reads at startup, so the answer is
/// the gateway's own: the launcher's alias table when this process was started
/// with it, and the route registry file, which defaults to the launcher's path
/// when the variable is not set so an operator shell sees the same file the
/// gateway does. The report names both, because a shell that has neither is
/// looking at the compiled-in contract alone and must say so.
pub fn alias_report() -> Result<AliasReport, std::io::Error> {
    let aliases = aliases_as_configured()?;
    let source = AliasReportSource {
        launcher_table_present: std::env::var_os(MODEL_ALIASES_ENV).is_some(),
        routes_file: aliases.routes_file.clone(),
    };
    let diagnoses = aliases
        .declared()
        .iter()
        .map(|alias| aliases.diagnose(alias))
        .collect();
    Ok(AliasReport {
        source,
        aliases: diagnoses,
    })
}

/// The route one alias resolves to for a reader that is not the running
/// gateway, with the diagnosis when it resolves to nothing.
///
/// `brama decide`, `brama image`, `brama video` and `brama speak` need
/// exactly what their endpoints need — a route the gateway can serve, or the
/// sentence saying why there is none — and they must read it the way the
/// gateway does rather than parse the registry themselves. Each shape
/// resolves through its own accessor, so an operator shell cannot reach a
/// shape over an alias the HTTP surface would refuse.
pub fn alias_routing(alias: &str) -> Result<(Option<String>, AliasDiagnosis), std::io::Error> {
    let aliases = aliases_as_configured()?;
    let route = if crate::core::server::DECISION_ALIASES.contains(&alias) {
        aliases.decision_route(alias)
    } else if alias == crate::core::server::IMAGE_ALIAS {
        aliases.image_route(alias)
    } else if alias == crate::core::server::VIDEO_ALIAS {
        aliases.video_route(alias)
    } else if alias == crate::core::server::VOICE_ALIAS {
        aliases.voice_route(alias)
    } else {
        aliases.chat_route(alias)
    };
    Ok((route, aliases.diagnose(alias)))
}

/// The alias table as this host is configured: the launcher's table when the
/// process was started with it, and the route registry, defaulting to the
/// launcher's path so an operator shell reads the file the gateway reads.
fn aliases_as_configured() -> Result<ModelAliases, std::io::Error> {
    if std::env::var_os(crate::core::inference_routes::ROUTES_FILE_ENV).is_none() {
        if let Some(home) = std::env::var_os("HOME") {
            let default_path = PathBuf::from(home).join(".config/brama/inference-routes.json");
            if default_path.is_file() {
                // Set only for this process: the CLI is reading, not serving.
                std::env::set_var(crate::core::inference_routes::ROUTES_FILE_ENV, default_path);
            }
        }
    }
    ModelAliases::from_env(false)
}
