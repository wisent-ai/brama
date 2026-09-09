//! Bringing the gateway up: what has to be true before a port is opened, the
//! background work that starts with it, and the address it binds.
//!
//! Everything refusable is refused here, before the listener exists — the
//! ingress table, the alias contract each first-party client depends on, and
//! the alias table itself — because a gateway that starts and then cannot serve
//! is harder to diagnose than one that never started and said why.

mod router;

use std::net::{Ipv4Addr, SocketAddr};

use tracing::info;

use crate::core::server::admission::ingress::ModelIngressAuth;
use crate::core::server::aliases::table::ModelAliases;
use crate::core::server::aliases::{BEST_ALIAS, WELES_ALIAS, WISENT_MODEL_ALIASES};
use crate::core::server::readiness::spawn_readiness_probe;
use crate::core::server::telemetry::STARTED_AT;
use crate::subscription_dispatch::plan_usage;

pub async fn start_server(port: u16, standalone: bool) -> Result<(), std::io::Error> {
    let _ = STARTED_AT.elapsed();
    let ingress_auth = ModelIngressAuth::from_env()?;
    if !standalone {
        ingress_auth.requires_aliases("wisent-backend", WISENT_MODEL_ALIASES)?;
        // Weles keeps `best` for its subscription-funded route and its own
        // alias, `weles`, for the model the operator selected for browser
        // tasks. Neither is a wildcard or a provider credential: both still
        // resolve through Brama's validated route table, so an operator can
        // point `weles` at local inference and `best` at the subscription that
        // pays without changing the caller or bypassing Brama.
        ingress_auth.requires_aliases("weles", &[BEST_ALIAS, WELES_ALIAS])?;
    }
    let aliases = ModelAliases::from_env(!standalone)?;
    // Touch the perf registry so persisted stats load at startup, not on first use.
    info!(
        models = crate::core::perf::tracked_count(),
        "perf registry loaded"
    );
    // Readiness crosses the local capability broker and provider discovery. It
    // runs out of band so a deploy probe always receives the latest completed
    // answer instead of canceling an in-flight check at its HTTP deadline.
    spawn_readiness_probe();
    // Plan usage is read from each provider's own usage report on a timer rather
    // than only from whatever traffic happens to arrive, so a subscription
    // nobody routed through today still reports what its plan says -- and no
    // timer spends a completion to find out.
    plan_usage::spawn();
    // A grant is replaced before it expires rather than when a request trips
    // over it, and a grant the provider has disowned is recorded the first time
    // it says so instead of being rediscovered by every later request.
    crate::subscription_dispatch::refresh_sweep::spawn();

    let app = router::app(aliases, ingress_auth);

    // Loopback unless this host is told otherwise. A gateway that binds a
    // routable address by default is one that can be reached before anybody
    // decided it should be, so the placed host declares its own address and
    // every other host keeps the old behaviour.
    let addr = match std::env::var("BRAMA_BIND_ADDRESS") {
        Ok(configured) if !configured.trim().is_empty() => {
            let parsed: std::net::IpAddr = configured.trim().parse().map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("BRAMA_BIND_ADDRESS is not an IP address: {configured}"),
                )
            })?;
            SocketAddr::from((parsed, port))
        }
        _ => SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
    };
    info!("Starting brama server on {addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
