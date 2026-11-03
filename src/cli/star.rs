//! The GitHub star request: after a person's fifth successful model call
//! (`test`, `decide`, `review`, `image`, `speak`, `music` or `video start`
//! answered by a provider) Brama asks once for a star on its public
//! repository. The request is the `star-request` journey every Wisent
//! product walks, run by the shared onboarding runtime and kept per user in
//! `$XDG_STATE_HOME/brama/onboarding.json` (default
//! `~/.local/state/brama/onboarding.json`); `brama star` reads, dismisses and
//! resets it. The threshold is the runtime's, the operator's number; the
//! repository is this package's `repository`, the catalog's
//! `star_repository`.

use clap::Subcommand;
use wisent_onboarding_client::{JourneyError, ProductStarRequest, StarProduct};

#[derive(Subcommand)]
pub(crate) enum StarCommand {
    /// Where the request stands: counting, shown or dismissed
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Never make the request again, until a reset
    Dismiss {
        #[arg(long)]
        json: bool,
    },
    /// Count again from zero; set aside a journey file that holds no state
    Reset {
        #[arg(long)]
        json: bool,
    },
}

fn request() -> Result<ProductStarRequest, JourneyError> {
    let product = StarProduct::from_package("brama", "Brama", env!("CARGO_PKG_REPOSITORY"))?;
    ProductStarRequest::new(product)
}

/// Count one model call a provider answered, when a person reads this
/// terminal. The request, on the call that makes it, goes to standard
/// error, so standard output and `--json` keep only the answer; a request
/// that cannot be kept is said there too, and the call's own answer stands.
pub(crate) fn observe() {
    match request() {
        Ok(request) => {
            request.observe_reporting();
        }
        Err(error) => {
            eprintln!("brama: this call is not counted toward the GitHub star request: {error}")
        }
    }
}

/// `brama star status|dismiss|reset [--json]`.
pub(crate) fn run(command: StarCommand) {
    let (verb, json) = match command {
        StarCommand::Status { json } => ("status", json),
        StarCommand::Dismiss { json } => ("dismiss", json),
        StarCommand::Reset { json } => ("reset", json),
    };
    let answered = request()
        .map_err(|error| error.to_string())
        .and_then(|request| request.command(&[verb.to_string()], json));
    match answered {
        Ok(text) => print!("{text}"),
        Err(refusal) => crate::cli::refused(refusal),
    }
}
