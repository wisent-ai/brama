//! What the HTTP edge asks for, and what that turns into: one named route, or
//! a selector that decides the route on the caller's behalf.
//!
//! `ranked_walk` is the walk itself -- a bounded number of provider round trips
//! down a ranked list, with a refusal that names every provider it went past.
//! `buffered` and `streaming` are the entrypoints for `any`, `best`,
//! `any-vision-capable`, `task:<name>` and a named subscription route, written
//! once per protocol so the two can never disagree about which candidate leads.

pub(super) mod buffered;
pub(super) mod ranked_walk;
pub(super) mod streaming;
