//! The catalogue facets and the media shapes, driven end to end.
//!
//! One target, two story files: the shell surface an operator reads the
//! catalogue and declares categories from, and the gateway surface where the
//! same facets are answered and the three media endpoints refuse a model
//! that cannot produce what was asked for.
//!
//! ```console
//! $ cargo test --test catalogue_real
//! ```
//!
//! No story here spends provider credit. Each one either reads the public
//! model catalogue, writes an isolated route registry, or is refused before
//! any provider is reached; a real render is a separate acknowledged call.

#[path = "../support/gateway.rs"]
mod gateway;
#[path = "../support/mod.rs"]
mod support;

mod facets;
mod media;
mod shell;
