//! The decision aliases, driven end to end.
//!
//! One target, four files: the fixture every story starts a real gateway
//! with, the served story that proves a real model answers inside the
//! declared schema, the refusals that keep a failure legible, and the shell
//! surface an operator declares and exercises the aliases from.
//!
//! ```console
//! $ cargo test --test decision_real
//! ```
//!
//! The served stories spend real provider credit, a few hundred tokens each:
//! the model answers numbers, not prose.

mod fixture;
mod refusals;
mod served;
mod surface;
