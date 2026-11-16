use crate::constants::*;
use crate::tools::error::ToolError;
use serde_json::Value;

pub(crate) fn require_object_input<'a>(
    input: &'a Value,
    tool_name: &str,
) -> Result<&'a serde_json::Map<String, Value>, ToolError> {
    input.as_object().ok_or_else(|| ToolError::NotAnObject {
        tool: tool_name.to_string(),
    })
}

pub(crate) fn require_string_field<'a>(
    input: &'a serde_json::Map<String, Value>,
    field: &str,
    context: &str,
) -> Result<&'a str, ToolError> {
    input
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingStringField {
            context: context.to_string(),
            field: field.to_string(),
        })
}

pub(crate) fn optional_string_field<'a>(
    input: &'a serde_json::Map<String, Value>,
    field: &str,
    context: &str,
) -> Result<Option<&'a str>, ToolError> {
    match input.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .map(Some)
            .ok_or_else(|| ToolError::MissingStringField {
                context: context.to_string(),
                field: field.to_string(),
            }),
    }
}

pub(crate) fn optional_bool_field(
    input: &serde_json::Map<String, Value>,
    field: &str,
    context: &str,
) -> Result<Option<bool>, ToolError> {
    match input.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_bool()
            .map(Some)
            .ok_or_else(|| ToolError::MissingBoolField {
                context: context.to_string(),
                field: field.to_string(),
            }),
    }
}

pub(crate) fn optional_bounded_usize_field(
    input: &serde_json::Map<String, Value>,
    field: &str,
    context: &str,
    default: usize,
    max: usize,
) -> Result<usize, ToolError> {
    let Some(value) = input.get(field).filter(|value| !value.is_null()) else {
        return Ok(default);
    };
    let raw = value
        .as_u64()
        .ok_or_else(|| ToolError::MissingIntegerField {
            context: context.to_string(),
            field: field.to_string(),
        })?;
    let parsed = usize::try_from(raw).map_err(|_| ToolError::IntegerFieldTooLarge {
        context: context.to_string(),
        field: field.to_string(),
    })?;
    if !(1..=max).contains(&parsed) {
        return Err(ToolError::IntegerFieldOutOfRange {
            context: context.to_string(),
            field: field.to_string(),
            max,
        });
    }
    Ok(parsed)
}

pub(crate) fn require_string_array_field(
    input: &serde_json::Map<String, Value>,
    field: &str,
    context: &str,
) -> Result<Vec<String>, ToolError> {
    let values =
        input
            .get(field)
            .and_then(Value::as_array)
            .ok_or_else(|| ToolError::MissingArrayField {
                context: context.to_string(),
                field: field.to_string(),
            })?;
    if values.is_empty() {
        return Err(ToolError::EmptyArrayField {
            context: context.to_string(),
            field: field.to_string(),
        });
    }

    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(ToString::to_string)
                .ok_or_else(|| ToolError::NonStringArrayItem {
                    context: context.to_string(),
                    field: field.to_string(),
                })
        })
        .collect()
}

pub(crate) fn require_i64_field(
    input: &serde_json::Map<String, Value>,
    field: &str,
    context: &str,
) -> Result<i64, ToolError> {
    input
        .get(field)
        .and_then(Value::as_i64)
        .ok_or_else(|| ToolError::MissingIntegerField {
            context: context.to_string(),
            field: field.to_string(),
        })
}

pub(crate) fn optional_timeout_ms(
    input: &serde_json::Map<String, Value>,
    max_ms: u64,
) -> Result<u64, ToolError> {
    let Some(timeout_ms) = input.get("timeout_ms").filter(|value| !value.is_null()) else {
        return Ok(DEFAULT_COMMAND_TIMEOUT_MS.min(max_ms));
    };
    let timeout_ms = timeout_ms
        .as_u64()
        .ok_or("run_command timeout_ms requires an integer field: timeout_ms")?;
    if !(1..=max_ms).contains(&timeout_ms) {
        return Err(ToolError::Message(format!(
            "run_command timeout_ms must be between 1 and {max_ms}; the user sets the limit with sandbox.command_timeout"
        )));
    }
    Ok(timeout_ms)
}
