//! Why one declared alias is in the state it is in, in the words an operator
//! repairs it with.

use serde::Serialize;

use super::table::ModelAliases;
use super::{alias_requires_direct_capability, BEST_ALIAS, DECISION_ALIASES, MODEL_ALIASES};

/// The alias resolves to a route this gateway can authenticate to.
pub const ALIAS_SERVING: &str = "serving";
/// The alias is known but no route is declared for it.
pub const ALIAS_NO_ROUTE: &str = "no_route";
/// The declared route names a provider this host holds no credential for.
pub const ALIAS_CAPABILITY_ABSENT: &str = "capability_absent";
/// The route registry file exists and cannot be read, so nothing in it serves.
pub const ALIAS_ROUTES_FILE_INVALID: &str = "routes_file_invalid";

/// One alias, what it points at, and whether that can be served right now.
#[derive(Clone, Debug, Serialize)]
pub struct AliasDiagnosis {
    pub alias: String,
    pub state: &'static str,
    pub route: Option<String>,
    pub reason: Option<String>,
    /// The alias is a selector the subscription pool resolves per caller, not
    /// a route this gateway can check on its own.
    ///
    /// `serving` for such an alias means the selector is declared, which is
    /// not the same as a credential existing: on 2026-09-21 `brama aliases`
    /// reported `best serving best` while every request for it was refused
    /// with `subscription_reauthorization_required`, because the pool held no
    /// live member. A report that cannot be told apart from a working gateway
    /// is what sent that diagnosis to the wrong place, so the flag is carried
    /// here and the pool's own count is printed beside it.
    #[serde(default)]
    pub subscription_resolved: bool,
}

impl AliasDiagnosis {
    pub fn serving(&self) -> bool {
        self.state == ALIAS_SERVING
    }
}

impl ModelAliases {
    /// Why one declared alias is in the state it is in.
    ///
    /// `source` and `chat_route` answer "give me a route or nothing", which is
    /// right for dispatch and useless for an operator: nothing looks the same
    /// whether the alias was never declared, points at a provider this host
    /// holds no credential for, or sits in a route file that stopped parsing.
    /// Those are three different repairs, so they are three different states.
    pub(in crate::core::server) fn diagnose(&self, alias: &str) -> AliasDiagnosis {
        // `best` is a selector, not a route: subscription dispatch resolves it
        // per caller identity, so no route table entry is expected and its
        // absence is not a fault. A `best` with an explicit route is an
        // operator override and is judged like any other route below.
        if alias == BEST_ALIAS && !matches!(self.declared_route(alias), Ok(Some(_))) {
            return AliasDiagnosis {
                alias: alias.to_string(),
                state: ALIAS_SERVING,
                route: Some(BEST_ALIAS.to_string()),
                reason: None,
                subscription_resolved: true,
            };
        }
        let declared = match self.declared_route(alias) {
            Ok(declared) => declared,
            Err(error) => {
                return AliasDiagnosis {
                    alias: alias.to_string(),
                    state: ALIAS_ROUTES_FILE_INVALID,
                    route: None,
                    reason: Some(format!(
                        "the inference route registry could not be read: {error}"
                    )),
                    subscription_resolved: false,
                };
            }
        };
        let Some(route) = declared else {
            return AliasDiagnosis {
                alias: alias.to_string(),
                state: ALIAS_NO_ROUTE,
                route: None,
                subscription_resolved: false,
                reason: Some(if MODEL_ALIASES.contains(&alias) {
                    format!(
                        "alias `{alias}` is required by this gateway but no route is declared for it; declare one with `PUT /v1/admin/routes` or in the launcher's model_aliases policy"
                    )
                } else if DECISION_ALIASES.contains(&alias) {
                    // Known by name, required of nobody: a deployment that
                    // serves no decisions declares no route, and a caller
                    // that asks anyway is told which name is unrouted rather
                    // than that its model name was wrong.
                    format!(
                        "alias `{alias}` is a decision alias this gateway knows but no route is declared for it; declare one with `brama routes set {alias} <provider/model>` or `PUT /v1/admin/routes`"
                    )
                } else {
                    format!("alias `{alias}` is not declared on this gateway")
                }),
            };
        };
        if !alias_requires_direct_capability(alias, &route)
            || crate::providers::adapter::provider_id_from_route(&route)
                .is_some_and(crate::gateway::broker::provider_capability_configured)
        {
            return AliasDiagnosis {
                alias: alias.to_string(),
                state: ALIAS_SERVING,
                route: Some(route),
                reason: None,
                subscription_resolved: false,
            };
        }
        let provider =
            crate::providers::adapter::provider_id_from_route(&route).unwrap_or_default();
        AliasDiagnosis {
            alias: alias.to_string(),
            state: ALIAS_CAPABILITY_ABSENT,
            reason: Some(format!(
                "alias `{alias}` routes to {route} but this gateway holds no provider credential for {provider}; issue the provider capability on this host or point the alias at a route it can reach, such as `best`"
            )),
            route: Some(route),
            subscription_resolved: false,
        }
    }
}
