//! The boot-time credential table: the clients this process was started with,
//! the refusals a malformed table must not survive, and the constant-time
//! bearer match that turns a header into one of them.

use std::collections::HashSet;

use axum::http::HeaderMap;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use super::identity::{ModelClientIdentity, ModelClientKind};
use super::presented_bearer;

pub(in crate::core::server) const MODEL_ROUTER_CLIENT_IDENTITIES_ENV: &str =
    "BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES";

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelClientCredential {
    client_id: String,
    token: String,
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    allowed_models: Option<Vec<String>>,
}

#[derive(Clone)]
struct ModelIngressCredential {
    identity: ModelClientIdentity,
    token_digest: sha2::digest::Output<Sha256>,
}

#[derive(Clone)]
pub(in crate::core::server) struct ModelIngressAuth {
    credentials: Vec<ModelIngressCredential>,
}

impl ModelIngressAuth {
    pub(in crate::core::server) fn from_env() -> Result<Self, std::io::Error> {
        // Absent means "ask the authority", not "misconfigured". Building this
        // table requires reading every client's secret before the first request
        // is served, which is a broad grant for a router and the reason a
        // gateway could not start on a host that was not provisioned to decrypt
        // other clients' items. A bearer that is not here is resolved against
        // Skarbiec, which is where it was issued.
        let Ok(encoded) = std::env::var(MODEL_ROUTER_CLIENT_IDENTITIES_ENV) else {
            return Ok(Self {
                credentials: Vec::new(),
            });
        };
        if encoded.trim().is_empty() {
            return Ok(Self {
                credentials: Vec::new(),
            });
        }
        let configured: Vec<ModelClientCredential> =
            serde_json::from_str(&encoded).map_err(|error| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("{MODEL_ROUTER_CLIENT_IDENTITIES_ENV} is invalid: {error}"),
                )
            })?;
        if configured.is_empty() {
            return Ok(Self {
                credentials: Vec::new(),
            });
        }

        let mut client_ids = HashSet::with_capacity(configured.len());
        let mut credentials = Vec::with_capacity(configured.len());
        for credential in configured {
            if credential.client_id.is_empty()
                || credential.client_id.trim() != credential.client_id
                || !credential
                    .client_id
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("{MODEL_ROUTER_CLIENT_IDENTITIES_ENV} contains an invalid client_id"),
                ));
            }
            if !client_ids.insert(credential.client_id.clone()) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "{MODEL_ROUTER_CLIENT_IDENTITIES_ENV} contains duplicate client_id {}",
                        credential.client_id
                    ),
                ));
            }
            if credential.token.is_empty()
                || credential.token.trim() != credential.token
                || credential
                    .token
                    .bytes()
                    .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "{MODEL_ROUTER_CLIENT_IDENTITIES_ENV} contains an invalid token for {}",
                        credential.client_id
                    ),
                ));
            }
            if credential.agent_id.as_deref().is_some_and(|agent_id| {
                agent_id.is_empty()
                    || agent_id.trim() != agent_id
                    || !agent_id.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
                    })
            }) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "{MODEL_ROUTER_CLIENT_IDENTITIES_ENV} contains an invalid agent_id for {}",
                        credential.client_id
                    ),
                ));
            }
            let allowed_models = credential
                .allowed_models
                .map(|models| {
                    if models.is_empty()
                        || models.iter().any(|model| {
                            model.is_empty()
                                || model.trim() != model
                                || model.contains('*')
                                || model.bytes().any(|byte| byte.is_ascii_whitespace())
                        })
                    {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            format!(
                                "{MODEL_ROUTER_CLIENT_IDENTITIES_ENV} contains invalid allowed_models for {}",
                                credential.client_id
                            ),
                        ));
                    }
                    let model_count = models.len();
                    let unique = models.into_iter().collect::<HashSet<_>>();
                    if unique.len() != model_count {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            format!(
                                "{MODEL_ROUTER_CLIENT_IDENTITIES_ENV} contains duplicate allowed_models for {}",
                                credential.client_id
                            ),
                        ));
                    }
                    Ok(unique)
                })
                .transpose()?;

            let token_digest = Sha256::digest(credential.token.as_bytes());
            if credentials.iter().any(|existing: &ModelIngressCredential| {
                existing
                    .token_digest
                    .as_slice()
                    .ct_eq(token_digest.as_slice())
                    .into()
            }) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("{MODEL_ROUTER_CLIENT_IDENTITIES_ENV} contains duplicate tokens"),
                ));
            }
            credentials.push(ModelIngressCredential {
                identity: ModelClientIdentity {
                    client_id: credential.client_id,
                    kind: ModelClientKind::Workload {
                        agent_id: credential.agent_id,
                    },
                    allowed_models,
                },
                token_digest,
            });
        }
        Ok(Self { credentials })
    }

    /// Every alias this build compiles in for `client_id` must be in the
    /// client's allowed set. The set may hold more: the launcher's policy is
    /// where an operator adds an alias, and a client that is allowed one name
    /// this build does not know is not misconfigured, it is ahead. Requiring
    /// equality here made every alias rename a flag day: the policy that let
    /// the new release start refused the release still running, and on
    /// 2026-09-05 0.2.75 died on this line while 0.2.72 could no longer be
    /// restarted under the same file, leaving nothing on port 18080.
    pub(in crate::core::server) fn requires_aliases(
        &self,
        client_id: &str,
        aliases: &[&str],
    ) -> Result<(), std::io::Error> {
        // This checks a table for internal consistency. With no table there is
        // nothing inconsistent: those clients are resolved against Skarbiec,
        // where their alias set is a capability rather than a line in an
        // environment variable this process was started with.
        if self.credentials.is_empty() {
            return Ok(());
        }
        let valid = self.credentials.iter().any(|credential| {
            credential.identity.client_id == client_id
                && credential
                    .identity
                    .allowed_models
                    .as_ref()
                    .is_some_and(|models| aliases.iter().all(|alias| models.contains(*alias)))
        });
        if valid {
            Ok(())
        } else {
            let mut required = aliases.to_vec();
            required.sort_unstable();
            let mut observed = self
                .credentials
                .iter()
                .find(|credential| credential.identity.client_id == client_id)
                .and_then(|credential| credential.identity.allowed_models.as_ref())
                .map(|models| models.iter().map(String::as_str).collect::<Vec<_>>());
            if let Some(models) = &mut observed {
                models.sort_unstable();
            }
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "{MODEL_ROUTER_CLIENT_IDENTITIES_ENV} must give `{client_id}` every required alias; required={required:?}; observed={observed:?}"
                ),
            ))
        }
    }

    pub(in crate::core::server) fn identity_for(
        &self,
        headers: &HeaderMap,
    ) -> Option<ModelClientIdentity> {
        let token = presented_bearer(headers)?;
        let presented_digest = Sha256::digest(token.as_bytes());
        let mut matched = None;
        for credential in &self.credentials {
            let equal: bool = credential
                .token_digest
                .as_slice()
                .ct_eq(presented_digest.as_slice())
                .into();
            if equal {
                matched = Some(credential.identity.clone());
            }
        }
        matched
    }
}
