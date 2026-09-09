//! `brama serve` and `brama mcp`: starting the surfaces this process serves.

use std::io::Read;

use clap::Args;
use tracing::info;

use brama::start_server;

#[derive(Args)]
pub(crate) struct ServeArgs {
    /// Port to listen on
    #[arg(short, long, default_value_t = 8080)]
    port: u16,
    /// Read a standalone provider-to-credential JSON object from stdin
    #[arg(long, default_value_t = false)]
    local_credentials_stdin: bool,
}

pub(crate) async fn serve(args: ServeArgs) {
    let ServeArgs {
        port,
        local_credentials_stdin,
    } = args;
    if local_credentials_stdin {
        let mut encoded = String::new();
        if let Err(error) = std::io::stdin().read_to_string(&mut encoded) {
            eprintln!("Server error: cannot read local credentials: {error}");
            std::process::exit(1);
        }
        if let Err(error) = brama::gateway::broker::install_local_provider_credentials(&mut encoded)
        {
            eprintln!("Server error: {error}");
            std::process::exit(1);
        }
    }
    info!("Starting server on port {port}");
    if let Err(e) = start_server(port, local_credentials_stdin).await {
        eprintln!("Server error: {e}");
        std::process::exit(1);
    }
}

pub(crate) fn mcp() {
    brama::mcp::serve();
}
