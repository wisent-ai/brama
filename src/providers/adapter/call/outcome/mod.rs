//! What came back from one provider call: the body as it was read, the refusal
//! it is turned into when it is not an answer, and whether the same call is
//! worth making again.
//!
//! Held together because a retry decision is made from a refusal, and a refusal
//! is made from a body.

pub(in crate::providers::adapter) mod refusal;
pub(in crate::providers::adapter) mod response_body;
pub(in crate::providers::adapter) mod retry;
