//! Bringing a tool schema into the shape providers actually accept.

use serde_json::{Value, json};

/// Some providers (e.g. Moonshot/Kimi) reject JSON Schemas whose nodes lack an
/// explicit `type`. Recursively infer a conservative `type` where it is
/// missing: object when `properties` is present, array when `items` is
/// present, the enum's first value kind, otherwise string.
///
/// A node is a schema by where it stands, not by which keys it carries: the
/// tool's `parameters`, each value of `properties`, each value of a map of
/// named schemas (`patternProperties`, `$defs`, `definitions`), and the
/// subschemas of the applicators. A map of named schemas is never itself
/// given a type. An empty schema accepts anything and a `$ref` takes its
/// type from the schema it names, so neither is given one.
fn needs_type(map: &serde_json::Map<String, Value>) -> bool {
    !map.is_empty() && !map.contains_key("type") && !map.contains_key("$ref")
}

/// Hand `visit` the value each of `keys` holds in `map`, skipping a key the
/// map does not hold.
fn visit_values(
    map: &mut serde_json::Map<String, Value>,
    keys: &[&str],
    mut visit: impl FnMut(&mut Value),
) {
    for key in keys {
        if let Some(child) = map.get_mut(*key) {
            visit(child);
        }
    }
}

fn normalize_json_schema(value: &mut Value) {
    if let Some(map) = value.as_object_mut() {
        // jeden's action schemas allow shorthand properties like
        // {"properties": {"type": "string"}} — a bare type name instead of a
        // schema object. Unwrap those before providers see them.
        if let Some(properties) = map.get_mut("properties").and_then(Value::as_object_mut) {
            for property in properties.values_mut() {
                if let Some(name) = property.as_str() {
                    let valid = matches!(
                        name,
                        "string" | "number" | "integer" | "boolean" | "object" | "array" | "null"
                    );
                    *property = json!({ "type": if valid { name } else { "string" } });
                }
                normalize_json_schema(property);
            }
        }
        if needs_type(map) {
            let inferred = if map.contains_key("properties") {
                "object"
            } else if map.contains_key("items") {
                "array"
            } else {
                match map
                    .get("enum")
                    .and_then(Value::as_array)
                    .and_then(|values| values.first())
                {
                    Some(Value::Number(_)) => "number",
                    Some(Value::Bool(_)) => "boolean",
                    _ => "string",
                }
            };
            map.insert("type".into(), Value::String(inferred.into()));
        }
        visit_values(
            map,
            &[
                "items",
                "additionalProperties",
                "anyOf",
                "oneOf",
                "allOf",
                "not",
            ],
            normalize_json_schema,
        );
        visit_values(
            map,
            &["patternProperties", "$defs", "definitions"],
            |schemas| {
                if let Some(schemas) = schemas.as_object_mut() {
                    schemas.values_mut().for_each(normalize_json_schema);
                }
            },
        );
    }
    if let Some(items) = value.as_array_mut() {
        for item in items {
            normalize_json_schema(item);
        }
    }
}

pub(in crate::providers::adapter) fn normalized_tools_value<T: serde::Serialize>(
    tools: &T,
) -> Value {
    let mut value = match serde_json::to_value(tools) {
        Ok(value) => value,
        Err(error) => unreachable!("a tool list serializes to JSON, yet: {error}"),
    };
    if let Some(array) = value.as_array_mut() {
        for tool in array {
            if let Some(parameters) = tool.pointer_mut("/function/parameters") {
                normalize_json_schema(parameters);
            }
        }
    }
    value
}
