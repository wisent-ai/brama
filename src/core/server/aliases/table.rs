//! The alias table this process serves from: the launcher's compiled-in policy
//! merged with the operator's route registry, and the one route each alias
//! resolves to.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use tracing::warn;

use super::{
    alias_requires_direct_capability, alias_route_shape_supported, MODEL_ALIASES,
    MODEL_ALIASES_ENV, WISENT_EMBEDDING_ALIAS, WISENT_MODERATION_ALIAS,
};

#[derive(Clone, Debug)]
pub(in crate::core::server) struct ModelAliases {
    pub(in crate::core::server) routes: HashMap<String, String>,
    pub(in crate::core::server) routes_file: Option<PathBuf>,
}

impl ModelAliases {
    pub(in crate::core::server) fn from_env(
        require_exact_aliases: bool,
    ) -> Result<Self, std::io::Error> {
        let encoded = match std::env::var(MODEL_ALIASES_ENV) {
            Ok(encoded) => encoded,
            Err(_) if !require_exact_aliases => "{}".to_owned(),
            Err(_) => {
                // A bare "is required" sent one supervised process into 3869
                // restarts against a cause no amount of retrying reaches: the
                // aliases are assembled by the launcher, so the binary started
                // on its own can never find them. Name the path that works.
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "{MODEL_ALIASES_ENV} is required and is assembled by \
                         src/release/bin/start-with-skarbiec from the sealed policy directory. \
                         Starting the binary directly cannot obtain it: launch the gateway \
                         through that launcher, or export the variable yourself. Restarting \
                         an unlaunched process will not repair this."
                    ),
                ));
            }
        };
        let routes: HashMap<String, String> = serde_json::from_str(&encoded).map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{MODEL_ALIASES_ENV} is invalid: {error}"),
            )
        })?;
        let routes_file = crate::core::inference_routes::configured_path();
        let mut effective_routes = routes.clone();
        if let Some(path) = routes_file.as_deref() {
            let dynamic = crate::core::inference_routes::resolved(path)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
            effective_routes.extend(dynamic);
        }
        // The named aliases are a contract with callers that ship against them,
        // so every one must be present. Equality went further and forbade any
        // additional name, which is why an operator could not add `smol` or
        // `best-vision` without editing this binary: the gateway refused to
        // start on the very alias the operator had just declared. Require the
        // named set, permit the rest.
        let required = MODEL_ALIASES
            .iter()
            .copied()
            .map(str::to_string)
            .collect::<HashSet<_>>();
        if require_exact_aliases {
            let declared = effective_routes.keys().cloned().collect::<HashSet<_>>();
            let missing = required.difference(&declared).cloned().collect::<Vec<_>>();
            if !missing.is_empty() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "{MODEL_ALIASES_ENV} is missing required alias(es): {}",
                        missing.join(", ")
                    ),
                ));
            }
        }
        for (alias, route) in effective_routes.iter() {
            if route.contains('*') || route.trim() != route {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("{MODEL_ALIASES_ENV} contains an unsafe route for {alias}"),
                ));
            }
            let supported = alias_route_shape_supported(alias.as_str(), route);
            if !supported {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "{MODEL_ALIASES_ENV} route for {alias} is not a shape this alias can take"
                    ),
                ));
            }
            // A missing provider capability and a malformed alias table are
            // different faults, and only one of them is this gateway's
            // configuration. The shape above is: refuse, because a wrong table
            // would route callers somewhere unintended. A credential that was
            // never issued on this host is the environment, and refusing to
            // start for it takes down every alias whose provider IS present --
            // which is how one absent subscription left the whole fleet with no
            // gateway at all. Start, serve what is serviceable, and let the
            // aliases that have no credential resolve to nothing, exactly as an
            // alias nobody declared already does.
            if alias_requires_direct_capability(alias.as_str(), route)
                && !crate::providers::adapter::provider_id_from_route(route)
                    .is_some_and(crate::gateway::broker::provider_capability_configured)
            {
                warn!(
                    event = "alias_provider_capability_absent",
                    alias = %alias,
                    route = %route,
                    "no provider capability was issued for this route; the alias will not serve"
                );
            }
        }
        Ok(Self {
            routes,
            routes_file,
        })
    }

    /// A route Brama can actually authenticate to, or nothing.
    ///
    /// Startup no longer refuses over a provider capability that was never
    /// issued here, so the check has to live where the route is used. Nothing
    /// is the honest answer: it is what an alias nobody declared already
    /// returns, and every caller of these two already handles it. Routing
    /// anyway would send a caller to a provider this gateway holds no
    /// credential for, and report it as that provider's refusal.
    fn serviceable(alias: &str, route: String) -> Option<String> {
        if !alias_requires_direct_capability(alias, &route) {
            return Some(route);
        }
        if crate::providers::adapter::provider_id_from_route(&route)
            .is_some_and(crate::gateway::broker::provider_capability_configured)
        {
            return Some(route);
        }
        warn!(
            event = "alias_route_unserviceable",
            alias = %alias,
            route = %route,
            "refusing an alias whose provider capability was never issued"
        );
        None
    }

    pub(in crate::core::server) fn source(&self, alias: &str) -> Option<String> {
        if let Some(path) = self.routes_file.as_deref() {
            match crate::core::inference_routes::resolve(path, alias) {
                Ok(Some(route)) => return Self::serviceable(alias, route),
                Ok(None) => {}
                Err(error) => {
                    warn!(event = "inference_routes_invalid", %error);
                    return None;
                }
            }
        }
        self.routes
            .get(alias)
            .cloned()
            .and_then(|route| Self::serviceable(alias, route))
    }

    /// The chat route for one alias, or nothing.
    ///
    /// Only the two aliases that promise a different capability are refused
    /// here. This was an allowlist of five chat names, which meant an
    /// operator-defined alias passed startup validation and then served
    /// nothing: `alias_route_shape_supported` accepted it and this returned
    /// `None` for it, so the alias existed and was permanently unroutable.
    pub(in crate::core::server) fn chat_route(&self, alias: &str) -> Option<String> {
        if matches!(alias, WISENT_EMBEDDING_ALIAS | WISENT_MODERATION_ALIAS) {
            return None;
        }
        if let Some(path) = self.routes_file.as_deref() {
            match crate::core::inference_routes::resolve(path, alias) {
                Ok(Some(route)) => return Self::serviceable(alias, route),
                Ok(None) => {}
                Err(error) => {
                    warn!(event = "inference_routes_invalid", %error);
                    return None;
                }
            }
        }
        self.routes
            .get(alias)
            .cloned()
            .and_then(|route| Self::serviceable(alias, route))
    }

    /// Every alias this gateway knows by name: the named contract plus every
    /// alias an operator declared in the launcher table or the route registry.
    pub(in crate::core::server) fn declared(&self) -> Vec<String> {
        let mut names = MODEL_ALIASES
            .iter()
            .map(|alias| (*alias).to_string())
            .collect::<Vec<_>>();
        names.extend(self.routes.keys().cloned());
        if let Some(path) = self.routes_file.as_deref() {
            if let Ok(dynamic) = crate::core::inference_routes::resolved(path) {
                names.extend(dynamic.into_keys());
            }
        }
        names.sort();
        names.dedup();
        names
    }

    pub(in crate::core::server) fn is_declared(&self, alias: &str) -> bool {
        MODEL_ALIASES.contains(&alias)
            || self.routes.contains_key(alias)
            || self.routes_file.as_deref().is_some_and(|path| {
                crate::core::inference_routes::resolve(path, alias)
                    .ok()
                    .flatten()
                    .is_some()
            })
    }

    /// The route as declared, before serviceability is applied.
    pub(in crate::core::server) fn declared_route(
        &self,
        alias: &str,
    ) -> Result<Option<String>, String> {
        if let Some(path) = self.routes_file.as_deref() {
            if let Some(route) = crate::core::inference_routes::resolve(path, alias)? {
                return Ok(Some(route));
            }
        }
        Ok(self.routes.get(alias).cloned())
    }
}
