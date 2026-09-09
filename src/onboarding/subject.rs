//! Which subject the journey is about: the workload whose first use this is.

use sha2::{Digest, Sha256};

use super::PRODUCT_ID;

pub(super) fn stable_subject_hash(agent_id: &str) -> String {
    let digest = Sha256::digest(format!("{PRODUCT_ID}:workload:{agent_id}").as_bytes());
    hex::encode(digest)
}
