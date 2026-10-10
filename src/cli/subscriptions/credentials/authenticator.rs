use clap::Subcommand;
#[derive(Subcommand)]
pub(crate) enum AuthenticatorCommand {
    /// List which accounts need a second factor to be signed in, and which of them hold the secret that answers one
    List {
        /// Narrow the list to one provider; without it every provider is listed
        #[arg(long)]
        provider: Option<String>,
        /// Print the list as JSON instead of lines
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Enrol an authenticator for the login behind one subscription, so every later sign-in answers the provider's second factor by itself
    Enrol {
        /// The provider whose account needs a seed: claude-code, codex or kimi
        provider: String,
        /// Exact Brama subscription whose login gains the authenticator
        #[arg(long)]
        subscription_id: String,
        /// Why this enrolment is being run; recorded beside the verdict
        #[arg(long)]
        reason: String,
        /// Exact Skarbiec login item, when the subscription names more than one
        #[arg(long)]
        login_item: Option<String>,
        /// Print the verdict as JSON instead of lines
        #[arg(long, default_value_t = false)]
        json: bool,
    },
}
