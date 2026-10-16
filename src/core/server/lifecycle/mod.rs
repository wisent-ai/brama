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

pub async fn start_server(port: u16, standalone: bool) -> Result<(), std::io::Error> {
    // Uptime counts from here, not from the first request that reads it.
    std::sync::LazyLock::force(&STARTED_AT);
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
    // Readiness crosses the local capability broker and provider discovery. Its
    // first reading is taken out of band so the listener opens at once; later
    // readings, plan usage reports and credential renewal are each one pass of
    // `brama maintain`, run by the host's Stado schedule.
    spawn_readiness_probe();

    let app = router::app(aliases, ingress_auth);

    // Loopback unless this host is told otherwise. A gateway that binds a
    // routable address by default is one that can be reached before anybody
    // decided it should be, so the placed host declares its own address and
    // every other host binds loopback. A declaration that is not Unicode, or
    // not an IP address, is refused by name rather than bound as loopback.
    let refused = |reason: String| std::io::Error::new(std::io::ErrorKind::InvalidInput, reason);
    let addr = match crate::environment::declared_setting("BRAMA_BIND_ADDRESS").map_err(refused)? {
        Some(configured) => {
            let parsed: std::net::IpAddr = configured.parse().map_err(|error| {
                refused(format!(
                    "BRAMA_BIND_ADDRESS is not an IP address: {configured}: {error}"
                ))
            })?;
            SocketAddr::from((parsed, port))
        }
        None => SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
    };
    info!("Starting brama server on {addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    // The bound address, not the requested one: `--port 0` asks the kernel
    // for a free port, and this line is how a supervisor or test learns it.
    info!("brama server listening on {}", listener.local_addr()?);
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
