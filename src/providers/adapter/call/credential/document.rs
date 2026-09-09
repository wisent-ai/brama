//! The wrapper the store handed a secret back in, and how to describe a
//! document that carries no credential without disclosing what it holds.

use serde_json::Value;

/// A typed envelope is exactly a `type` and a `value`, and nothing else. An
/// object that merely carries a `type` beside its own fields -- a reauth
/// document does -- is a credential, not a container.
const ENVELOPE_FIELDS: usize = 2;

/// The credential behind whatever the store handed back.
///
/// Two packagings reach here and both are legitimate. A credential written
/// directly is the secret itself, or a provider document carrying it under a
/// known field. A credential imported into Skarbiec is wrapped in a typed
/// envelope -- `{"type": "env", "value": "..."}` -- with the secret one level
/// in, and the store hands that back verbatim.
///
/// Nothing unwrapped it, so every item written by the import path answered
/// `no supported key field`: a sentence that reads as an absent credential
/// while the credential was present, in a container. The peel happens here,
/// on the one path every provider and every subscription shares, rather than
/// in the branch of whichever provider was noticed first.
///
/// `None` means the text is not JSON at all, which is the ordinary shape of a
/// bare secret and needs no interpretation.
pub(super) fn credential_document(secret: &str) -> Option<Value> {
    let mut value = serde_json::from_str::<Value>(secret.trim()).ok()?;
    // Envelopes nest when an already-wrapped value is imported a second time,
    // so peel until the shape stops being one.
    loop {
        let inner = match value.as_object_mut() {
            Some(fields) if fields.len() == ENVELOPE_FIELDS && fields.contains_key("type") => {
                fields.remove("value")
            }
            _ => None,
        };
        match inner {
            Some(inner) => value = inner,
            None => return Some(value),
        }
    }
}

/// Say what an unusable credential looks like without saying what it holds.
///
/// Field names are structure, not secrets, and naming them is the difference
/// between an operator finding the item in one look and guessing at it.
pub(super) fn credential_shape(document: &Value) -> String {
    match document {
        Value::Object(fields) => {
            let mut names = fields.keys().map(String::as_str).collect::<Vec<_>>();
            names.sort_unstable();
            format!("a JSON object with fields [{}]", names.join(", "))
        }
        Value::Array(items) => format!("a JSON array of {} entries", items.len()),
        Value::String(_) => "an empty JSON string".to_string(),
        Value::Number(_) => "a bare JSON number".to_string(),
        Value::Bool(_) => "a bare JSON boolean".to_string(),
        Value::Null => "JSON null".to_string(),
    }
}
