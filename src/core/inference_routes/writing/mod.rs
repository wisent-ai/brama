//! Changing the route file that is already there: an operator edit, including
//! the migration onto the current document shape, and the merge of a reviewed
//! configuration into it.
//!
//! Both write through the same lock and the same validation the document
//! module owns, which is why they sit together and apart from reading.

pub mod editing;
pub mod importing;
