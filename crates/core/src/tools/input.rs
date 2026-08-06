use crate::constants::*;
use serde_json::Value;

pub(crate) fn require_object_input<'a>(
    input: &'a Value,
    tool_name: &str,
) -> Result<&'a serde_json::Map<String, Value>, String> {
    input
        .as_object()
        .ok_or_else(|| format!("{tool_name} requires an object input"))
}

pub(crate) fn require_string_field<'a>(
    input: &'a serde_json::Map<String, Value>,
    field: &str,
    context: &str,
) -> Result<&'a str, String> {
    input
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{context} requires a string field: {field}"))
}

pub(crate) fn optional_string_field<'a>(
    input: &'a serde_json::Map<String, Value>,
    field: &str,
    context: &str,
) -> Result<Option<&'a str>, String> {
    match input.get(field) {
        None => Ok(None),
        Some(value) => value
            .as_str()
            .map(Some)
            .ok_or_else(|| format!("{context} requires a string field: {field}")),
    }
}

pub(crate) fn optional_bool_field(
    input: &serde_json::Map<String, Value>,
    field: &str,
    context: &str,
) -> Result<Option<bool>, String> {
    match input.get(field) {
        None => Ok(None),
        Some(value) => value
            .as_bool()
            .map(Some)
            .ok_or_else(|| format!("{context} requires a boolean field: {field}")),
    }
}

pub(crate) fn optional_bounded_usize_field(
    input: &serde_json::Map<String, Value>,
    field: &str,
    context: &str,
    default: usize,
    max: usize,
) -> Result<usize, String> {
    let Some(value) = input.get(field) else {
        return Ok(default);
    };
    let raw = value
        .as_u64()
        .ok_or_else(|| format!("{context} requires an integer field: {field}"))?;
    let parsed = usize::try_from(raw)
        .map_err(|_| format!("{context} field '{field}' is too large for this platform"))?;
    if !(1..=max).contains(&parsed) {
        return Err(format!(
            "{context} field '{field}' must be between 1 and {max}"
        ));
    }
    Ok(parsed)
}

pub(crate) fn require_string_array_field(
    input: &serde_json::Map<String, Value>,
    field: &str,
    context: &str,
) -> Result<Vec<String>, String> {
    let values = input
        .get(field)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{context} requires an array field: {field}"))?;
    if values.is_empty() {
        return Err(format!(
            "{context} requires a non-empty array field: {field}"
        ));
    }

    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(ToString::to_string)
                .ok_or_else(|| format!("{context} requires string array items in field: {field}"))
        })
        .collect()
}

pub(crate) fn require_i64_field(
    input: &serde_json::Map<String, Value>,
    field: &str,
    context: &str,
) -> Result<i64, String> {
    input
        .get(field)
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("{context} requires an integer field: {field}"))
}

pub(crate) fn optional_timeout_ms(input: &serde_json::Map<String, Value>) -> Result<u64, String> {
    let Some(timeout_ms) = input.get("timeout_ms") else {
        return Ok(DEFAULT_COMMAND_TIMEOUT_MS);
    };
    let timeout_ms = timeout_ms
        .as_u64()
        .ok_or("run_command timeout_ms requires an integer field: timeout_ms")?;
    if !(1..=MAX_COMMAND_TIMEOUT_MS).contains(&timeout_ms) {
        return Err(format!(
            "run_command timeout_ms must be between 1 and {MAX_COMMAND_TIMEOUT_MS}"
        ));
    }
    Ok(timeout_ms)
}
