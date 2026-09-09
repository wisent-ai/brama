//! Bringing a tool schema into the shape providers actually accept.

use serde_json::{json, Value};

/// Some providers (e.g. Moonshot/Kimi) reject JSON Schemas whose nodes lack an
/// explicit `type`. Recursively infer a conservative `type` where it is
/// missing: object when `properties` is present, array when `items` is
/// present, the enum's first value kind, otherwise string.
/// Keys whose presence marks an object as a JSON Schema node (as opposed to a
/// plain map like the `properties` object itself).
const SCHEMA_HINT_KEYS: &[&str] = &[
    "enum",
    "const",
    "items",
    "properties",
    "required",
    "additionalProperties",
    "anyOf",
    "oneOf",
    "allOf",
    "not",
    "patternProperties",
    "description",
    "format",
    "pattern",
    "default",
    "examples",
    "minimum",
    "maximum",
    "minLength",
    "maxLength",
    "minItems",
    "maxItems",
    "uniqueItems",
    "multipleOf",
];

fn is_schema_node(map: &serde_json::Map<String, Value>) -> bool {
    SCHEMA_HINT_KEYS.iter().any(|key| map.contains_key(*key))
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
        if !map.contains_key("type") && is_schema_node(map) {
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
        for key in [
            "items",
            "additionalProperties",
            "anyOf",
            "oneOf",
            "allOf",
            "not",
            "patternProperties",
            "$defs",
            "definitions",
        ] {
            if let Some(child) = map.get_mut(key) {
                normalize_json_schema(child);
            }
        }
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
    let mut value = serde_json::to_value(tools).unwrap_or(Value::Null);
    if let Some(array) = value.as_array_mut() {
        for tool in array {
            if let Some(parameters) = tool.pointer_mut("/function/parameters") {
                normalize_json_schema(parameters);
            }
        }
    }
    value
}
