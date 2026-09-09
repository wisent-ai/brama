//! Who is asking, and on whose behalf: the two identifiers every adoption
//! request carries, held to shapes that can be printed, logged and compared
//! without escaping.

use super::{MAX_AGENT_ID_CHARACTERS, MAX_SOURCE_NAME_CHARACTERS};

pub(super) fn validate_request_identity(source_name: &str, agent_id: &str) -> Result<(), String> {
    if source_name.is_empty()
        || source_name.chars().count() > MAX_SOURCE_NAME_CHARACTERS
        || source_name.chars().any(char::is_control)
    {
        return Err("configuration source name is invalid".to_string());
    }
    if agent_id.is_empty()
        || agent_id.chars().count() > MAX_AGENT_ID_CHARACTERS
        || agent_id.bytes().any(|byte| {
            !(byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'-' | b'_' | b'.'))
        })
    {
        return Err("agent id is invalid".to_string());
    }
    Ok(())
}
