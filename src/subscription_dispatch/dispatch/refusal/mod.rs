//! The sentences a refused model request is answered with, and the envelope
//! that carries them into the log.
//!
//! Every path in this module refuses in the same words: `envelope` writes the
//! one refusal shape, and `pool_empty` holds the four sentences an emptied
//! credential pool is reported with. They live together because a second
//! wording of the same fault makes one broken chain read as two, which is the
//! defect the sentences here were written to end.

pub(super) mod envelope;
pub(super) mod pool_empty;
