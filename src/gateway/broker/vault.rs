//! The seam between this gateway and the local Skarbiec.
//!
//! Everything on the far side of one child process lives here, split by what
//! that process is asked to do rather than by who asks: `router` runs the
//! binary and turns its exit into a sentence, `capability` obtains an issuance
//! from the authority, `grant` reads a field through the route table an
//! operator wrote, and `item` is one vault row -- what it carries and what a
//! credential write puts there.
//!
//! Nothing here decides whether a credential is wanted; the callers above do
//! that, and this answers only with what the vault said.

mod capability;
mod grant;
mod item;
mod router;
mod seeds;

// The names the rest of `broker` calls, listed one at a time so no path
// resolves to more of this seam than its caller needs.
pub(super) use capability::{issue_capability, PROVIDER_PURPOSE, REQUEST_SIGN_PURPOSE};
pub(super) use grant::credential_by_grant;
pub(super) use item::{existing_item_account, existing_item_tags, put_credential, VaultListItem};
/// The vault program itself is named crate-wide: the sign-in path reads
/// `brama-weles-reauth` through the same program every credential operation
/// runs, and a second answer to "which vault binary" is how one caller ends
/// up spawning a name no machine installs.
pub(crate) use router::entitlements_router_bin;
pub(super) use router::raw_listing;
pub(crate) use seeds::{login_seed_present, login_seed_states};
