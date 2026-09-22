//! The route registry as the console changes it: writing a route, adopting an
//! existing configuration, and declaring or retiring a category.
//!
//! All three touch the same file through the same lock, which is why they sit
//! together and apart from the credential store and the read-only snapshot.

pub(in crate::core::server) mod adoption;
pub(in crate::core::server) mod categories;
pub(in crate::core::server) mod routes;
