//! In what order a signed agent's routes and accounts are tried.
//!
//! `plan_order` ranks by what the ledger says each plan window has left, with
//! chance breaking only genuine ties; `pin` remembers which account served this
//! agent last, so consecutive turns reach the same prompt cache; `candidates`
//! builds the lists `any`, `best` and `any-vision-capable` walk; and
//! `task_quality` replaces that ranking with measured evidence for one named
//! task, keeping plan headroom as the tie-break inside a score.

pub(super) mod candidates;
pub(super) mod pin;
pub(super) mod plan_order;
pub(super) mod task_quality;
