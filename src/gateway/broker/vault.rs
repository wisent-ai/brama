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

// The names the rest of `broker` calls, listed one at a time so no path
// resolves to more of this seam than its caller needs.
pub(super) use capability::{issue_capability, PROVIDER_PURPOSE, REQUEST_SIGN_PURPOSE};
pub(super) use grant::credential_by_grant;
pub(super) use item::{existing_item_tags, put_credential, VaultListItem};
pub(super) use router::{bounded_output, entitlements_router_bin, router_output, router_refusal};
