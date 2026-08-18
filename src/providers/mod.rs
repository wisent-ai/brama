//! External model-provider integration boundary.
//!
//! Core routing selects an eligible provider-neutral request. `adapter` owns
//! provider inventory, endpoint/auth policy, wire translation, discovery,
//! bounded HTTP execution, and normalized provider responses.

pub mod adapter;
pub mod stream;
