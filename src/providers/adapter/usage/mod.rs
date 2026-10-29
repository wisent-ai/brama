//! The token usage a provider answer states, read the same way for every
//! dialect: both counts, or none when the answer leaves either out.

use serde_json::Value;

use crate::types::TokenUsage;

/// The token usage a provider answer states: both counts, `None` when it
/// leaves either out (no measurement, never zero tokens), or a refusal naming
/// a count that is present but not a whole number that fits.
pub(crate) fn stated_usage(
    input: Option<&Value>,
    output: Option<&Value>,
) -> Result<Option<TokenUsage>, String> {
    let count = |value: Option<&Value>, side: &str| -> Result<Option<u32>, String> {
        let Some(stated) = value.filter(|value| !value.is_null()) else {
            return Ok(None);
        };
        let Some(number) = stated.as_u64() else {
            return Err(format!(
                "the provider stated {side} tokens as {stated}, not a whole number"
            ));
        };
        u32::try_from(number).map(Some).map_err(|error| {
            format!("the provider stated {number} {side} tokens, more than a count holds: {error}")
        })
    };
    Ok(match (count(input, "input")?, count(output, "output")?) {
        (Some(input_tokens), Some(output_tokens)) => Some(TokenUsage {
            input_tokens,
            output_tokens,
        }),
        _ => None,
    })
}
