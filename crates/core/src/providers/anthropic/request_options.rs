use crate::models::ModelFeatures;
use serde_json::{Value, json};
use std::io::{Error, ErrorKind};

pub(super) const THINKING_DISPLAY_UPDATES_BETA: &str = "thinking-display-updates-2026-08-18";

/// How much of the model's adaptive thinking the API returns as text.
/// Thinking itself, and its billing, is the same under every setting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ThinkingDisplay {
    /// Reasoning summaries and between-tool progress updates, mixed.
    #[default]
    Summarized,
    /// Progress updates only; reasoning stays hidden. Requires a beta header.
    Updates,
    /// Thinking blocks stream with empty text.
    Omitted,
}

impl ThinkingDisplay {
    fn as_str(self) -> &'static str {
        match self {
            Self::Summarized => "summarized",
            Self::Updates => "updates",
            Self::Omitted => "omitted",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RequestOptions {
    pub(crate) thinking_display: ThinkingDisplay,
    pub(crate) effort: Option<&'static str>,
    /// Problems with the configuration that did not prevent startup.
    pub(crate) warnings: Vec<String>,
}

const EFFORT_LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

impl RequestOptions {
    /// Reads `THINKING_DISPLAY` and `EFFORT` through `lookup` so tests can
    /// supply values without touching the process environment.
    pub(crate) fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, Error> {
        let thinking_display = match lookup("THINKING_DISPLAY").as_deref().map(str::trim) {
            None | Some("") => ThinkingDisplay::default(),
            Some(raw) => match raw.to_ascii_lowercase().as_str() {
                "summarized" => ThinkingDisplay::Summarized,
                "updates" => ThinkingDisplay::Updates,
                "omitted" => ThinkingDisplay::Omitted,
                _ => {
                    return Err(Error::new(
                        ErrorKind::InvalidInput,
                        format!(
                            "THINKING_DISPLAY must be summarized, updates, or omitted; got '{raw}'"
                        ),
                    ));
                }
            },
        };
        let effort = match lookup("EFFORT").as_deref().map(str::trim) {
            None | Some("") => None,
            Some(raw) => {
                let lowered = raw.to_ascii_lowercase();
                Some(
                    *EFFORT_LEVELS
                        .iter()
                        .find(|level| **level == lowered)
                        .ok_or_else(|| {
                            Error::new(
                                ErrorKind::InvalidInput,
                                format!(
                                    "EFFORT must be one of {}; got '{raw}'",
                                    EFFORT_LEVELS.join(", ")
                                ),
                            )
                        })?,
                )
            }
        };
        let mut warnings = Vec::new();
        if lookup("THINKING_BUDGET_TOKENS").is_some() {
            warnings.push(
                "THINKING_BUDGET_TOKENS is no longer supported and is ignored; set EFFORT to control thinking depth"
                    .to_string(),
            );
        }
        Ok(Self {
            thinking_display,
            effort,
            warnings,
        })
    }

    /// Records settings the model cannot honor, so they are reported once
    /// instead of silently dropped.
    pub(crate) fn warn_unsupported(&mut self, model: &str, features: ModelFeatures) {
        if self.thinking_display != ThinkingDisplay::default() && !features.adaptive_thinking {
            self.warnings.push(format!(
                "THINKING_DISPLAY is ignored for {model}: it does not list the adaptive_thinking feature"
            ));
        }
        if self.effort.is_some() && !features.effort {
            self.warnings.push(format!(
                "EFFORT is ignored for {model}: it does not list the effort feature"
            ));
        }
    }

    /// Adds the Anthropic extensions that `features` allows.
    pub(super) fn apply(&self, body: &mut Value, features: ModelFeatures) {
        if features.adaptive_thinking {
            body["thinking"] = json!({
                "type": "adaptive",
                "display": self.thinking_display.as_str(),
            });
        }
        if let Some(effort) = self.effort.filter(|_| features.effort) {
            body["output_config"] = json!({ "effort": effort });
        }
        // Stream tool inputs as they are generated so the Activity pane can
        // show intent early. Anthropic-defined tools have no input schema and
        // keep their default buffering. Execution still waits for the
        // complete, parsed input.
        if features.eager_input_streaming
            && let Some(tools) = body["tools"].as_array_mut()
        {
            for tool in tools
                .iter_mut()
                .filter(|tool| tool.get("input_schema").is_some())
            {
                tool["eager_input_streaming"] = Value::Bool(true);
            }
        }
    }

    pub(super) fn beta_header(&self, features: ModelFeatures) -> Option<&'static str> {
        (features.adaptive_thinking && matches!(self.thinking_display, ThinkingDisplay::Updates))
            .then_some(THINKING_DISPLAY_UPDATES_BETA)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(pairs: &[(&str, &str)]) -> Result<RequestOptions, Error> {
        RequestOptions::from_lookup(|key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.to_string())
        })
    }

    #[test]
    fn defaults_to_summarized_adaptive_thinking_without_effort_or_beta() {
        let options = options(&[]).unwrap();
        let mut body = json!({"model": "claude-opus-5-5"});
        options.apply(&mut body, ModelFeatures::ALL);

        assert_eq!(
            body["thinking"],
            json!({"type": "adaptive", "display": "summarized"})
        );
        assert!(body.get("output_config").is_none());
        assert_eq!(options.beta_header(ModelFeatures::ALL), None);
        assert!(options.warnings.is_empty());
    }

    #[test]
    fn updates_display_requires_the_beta_header() {
        let options = options(&[("THINKING_DISPLAY", "Updates")]).unwrap();
        let mut body = json!({});
        options.apply(&mut body, ModelFeatures::ALL);

        assert_eq!(body["thinking"]["display"], "updates");
        assert_eq!(
            options.beta_header(ModelFeatures::ALL),
            Some(THINKING_DISPLAY_UPDATES_BETA)
        );
        assert_eq!(
            self::options(&[("THINKING_DISPLAY", "omitted")])
                .unwrap()
                .beta_header(ModelFeatures::ALL),
            None
        );
    }

    #[test]
    fn effort_is_sent_inside_output_config() {
        let options = options(&[("EFFORT", " XHigh ")]).unwrap();
        let mut body = json!({});
        options.apply(&mut body, ModelFeatures::ALL);

        assert_eq!(body["output_config"], json!({"effort": "xhigh"}));
    }

    #[test]
    fn custom_tools_stream_their_input_eagerly() {
        let mut body = json!({"tools": [
            {"name": "read_file", "description": "", "input_schema": {"type": "object"}},
            {"type": "text_editor_20250728", "name": "str_replace_based_edit_tool"},
        ]});
        RequestOptions::default().apply(&mut body, ModelFeatures::ALL);

        assert_eq!(body["tools"][0]["eager_input_streaming"], true);
        assert!(body["tools"][1].get("eager_input_streaming").is_none());
    }

    #[test]
    fn rejects_unknown_display_and_effort_values() {
        let display = options(&[("THINKING_DISPLAY", "full")]).unwrap_err();
        assert_eq!(display.kind(), ErrorKind::InvalidInput);
        assert!(display.to_string().contains("THINKING_DISPLAY"));

        let effort = options(&[("EFFORT", "extreme")]).unwrap_err();
        assert!(effort.to_string().contains("low, medium, high, xhigh, max"));
    }

    #[test]
    fn budget_tokens_is_ignored_with_a_warning() {
        let options = options(&[("THINKING_BUDGET_TOKENS", "4096")]).unwrap();
        let mut body = json!({});
        options.apply(&mut body, ModelFeatures::ALL);

        assert_eq!(body["thinking"]["type"], "adaptive");
        assert!(body["thinking"].get("budget_tokens").is_none());
        assert_eq!(options.warnings.len(), 1);
        assert!(options.warnings[0].contains("EFFORT"));
    }

    #[test]
    fn models_without_features_receive_no_anthropic_extensions() {
        let options = options(&[("THINKING_DISPLAY", "updates"), ("EFFORT", "high")]).unwrap();
        let mut body = json!({"tools": [
            {"name": "read_file", "description": "", "input_schema": {"type": "object"}},
        ]});
        options.apply(&mut body, ModelFeatures::default());

        assert!(body.get("thinking").is_none());
        assert!(body.get("output_config").is_none());
        assert!(body["tools"][0].get("eager_input_streaming").is_none());
        assert_eq!(options.beta_header(ModelFeatures::default()), None);
    }

    #[test]
    fn settings_a_model_cannot_honor_are_warned_about() {
        let mut options = options(&[("THINKING_DISPLAY", "omitted"), ("EFFORT", "high")]).unwrap();
        options.warn_unsupported("ds-flash", ModelFeatures::default());
        assert_eq!(options.warnings.len(), 2);
        assert!(options.warnings[0].contains("THINKING_DISPLAY is ignored for ds-flash"));
        assert!(options.warnings[1].contains("EFFORT is ignored for ds-flash"));

        let mut quiet = self::options(&[("EFFORT", "high")]).unwrap();
        quiet.warn_unsupported("opus", ModelFeatures::ALL);
        assert!(quiet.warnings.is_empty());
    }
}
