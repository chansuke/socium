//! The `/model` picker shown in place of the composer, alias completion,
//! and switching the agent's model.

use crate::app::App;
use crate::transcript::Speaker;
use ratatui::crossterm::event::KeyCode;
use socium_core::{ModelChoice, ModelSwitch};

pub(crate) struct ModelPicker {
    pub(crate) choices: Vec<ModelChoice>,
    pub(crate) selected: usize,
}

/// `provider/model-id`, as the footer and picker show a model.
pub(crate) fn model_label(choice: &ModelChoice) -> String {
    format!("{}/{}", choice.provider, choice.model_id)
}

const MODEL_COMMAND_PREFIX: &str = "/model ";

/// Completes `partial` to the longest prefix shared by the aliases that
/// start with it, and returns those aliases.
fn complete_alias<'a>(partial: &str, aliases: &'a [String]) -> (String, Vec<&'a str>) {
    let matches = aliases
        .iter()
        .map(String::as_str)
        .filter(|alias| alias.starts_with(partial))
        .collect::<Vec<_>>();
    let Some(first) = matches.first() else {
        return (partial.to_string(), matches);
    };
    let mut common = first.to_string();
    for alias in &matches[1..] {
        let shared = common
            .chars()
            .zip(alias.chars())
            .take_while(|(a, b)| a == b)
            .map(|(a, _)| a.len_utf8())
            .sum();
        common.truncate(shared);
    }
    (common, matches)
}

impl App {
    pub(crate) fn open_model_picker(&mut self, choices: Vec<ModelChoice>) {
        let selected = choices.iter().position(|choice| choice.active).unwrap_or(0);
        self.model_picker = Some(ModelPicker { choices, selected });
        self.status = Some("Model: ↑↓ select · Enter switch · Esc close".into());
    }

    /// Tab after `/model `: completes the alias, listing the candidates in
    /// the status line when more than one remains. Returns whether the
    /// input was a `/model` argument.
    pub(crate) fn complete_model_alias(&mut self) -> bool {
        let Some(partial) = self.input.strip_prefix(MODEL_COMMAND_PREFIX) else {
            return false;
        };
        let partial = partial.trim_start();
        if partial.contains(char::is_whitespace) {
            return false;
        }
        let aliases = self
            .agent
            .as_ref()
            .map(|agent| {
                agent
                    .available_models()
                    .into_iter()
                    .map(|choice| choice.alias)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let (completed, matches) = complete_alias(partial, &aliases);
        self.status = Some(match matches.as_slice() {
            [] => format!("No model alias starts with '{partial}'"),
            [_] => "Enter to switch".to_string(),
            several => several.join("  "),
        });
        self.input = format!("{MODEL_COMMAND_PREFIX}{completed}");
        self.character_index = self.input.chars().count();
        true
    }

    /// Returns `true` when a switch asks for the history to be compacted.
    pub(crate) fn handle_model_picker_key(&mut self, code: KeyCode) -> bool {
        let Some(picker) = self.model_picker.as_mut() else {
            return false;
        };
        let last = picker.choices.len().saturating_sub(1);
        match code {
            KeyCode::Up | KeyCode::Char('k') => picker.selected = picker.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => picker.selected = (picker.selected + 1).min(last),
            KeyCode::Esc => {
                self.model_picker = None;
                self.status = Some("Model unchanged".into());
            }
            KeyCode::Char(digit @ '1'..='9') => {
                let index = digit as usize - '1' as usize;
                if index <= last {
                    picker.selected = index;
                    return self.confirm_model_choice();
                }
            }
            KeyCode::Enter => return self.confirm_model_choice(),
            _ => {}
        }
        false
    }

    fn confirm_model_choice(&mut self) -> bool {
        let Some(choice) = self
            .model_picker
            .as_ref()
            .and_then(|picker| picker.choices.get(picker.selected).cloned())
        else {
            return false;
        };
        if let Some(reason) = &choice.unavailable {
            self.status = Some(format!("{} is unavailable: {reason}", choice.alias));
            return false;
        }
        self.model_picker = None;
        self.switch_model(&choice.alias)
    }

    /// Returns `true` when the history should be compacted for the new
    /// model before the next turn.
    pub(crate) fn switch_model(&mut self, alias: &str) -> bool {
        let Some(agent) = self.agent.as_mut() else {
            self.status = Some("Request in progress".into());
            return false;
        };
        match agent.switch_model(alias) {
            Ok(switch) => self.show_model_switch(switch),
            Err(err) => {
                self.push_message(Speaker::System, format!("Cannot switch to {alias}: {err}"));
                self.status = Some("Model unchanged".into());
                false
            }
        }
    }

    fn show_model_switch(&mut self, switch: ModelSwitch) -> bool {
        let ModelSwitch {
            model,
            warnings,
            needs_compaction,
        } = switch;
        self.model_label = model_label(&model);
        self.push_message(
            Speaker::System,
            format!("Switched to {} ({})", model.alias, self.model_label),
        );
        for warning in warnings {
            self.push_message(Speaker::System, warning);
        }
        self.status = Some(format!("Model: {}", model.alias));
        self.refresh_context_metrics();
        if needs_compaction {
            self.push_message(Speaker::System, "Compacting conversation context…");
            self.status = Some("Compacting".into());
        }
        needs_compaction
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice(alias: &str, active: bool, unavailable: Option<&str>) -> ModelChoice {
        ModelChoice {
            alias: alias.into(),
            provider: "p".into(),
            model_id: format!("{alias}-id"),
            description: None,
            active,
            unavailable: unavailable.map(str::to_string),
        }
    }

    fn app_with_picker() -> App {
        let mut app = App::for_test();
        app.open_model_picker(vec![
            choice("opus", false, None),
            choice("sonnet", true, None),
            choice("ds-flash", false, Some("DEEPSEEK_API_KEY is not set")),
        ]);
        app
    }

    #[test]
    fn the_picker_opens_on_the_active_model_and_moves_within_bounds() {
        let mut app = app_with_picker();
        assert_eq!(app.model_picker.as_ref().unwrap().selected, 1);
        app.handle_model_picker_key(KeyCode::Up);
        app.handle_model_picker_key(KeyCode::Up);
        assert_eq!(app.model_picker.as_ref().unwrap().selected, 0);
        for _ in 0..5 {
            app.handle_model_picker_key(KeyCode::Down);
        }
        assert_eq!(app.model_picker.as_ref().unwrap().selected, 2);
    }

    #[test]
    fn an_unavailable_model_keeps_the_picker_open_with_the_reason() {
        let mut app = app_with_picker();
        app.handle_model_picker_key(KeyCode::Down);
        app.handle_model_picker_key(KeyCode::Enter);
        assert!(app.model_picker.is_some());
        assert_eq!(
            app.status.as_deref(),
            Some("ds-flash is unavailable: DEEPSEEK_API_KEY is not set")
        );
    }

    #[test]
    fn alias_completion_extends_to_the_shared_prefix() {
        let aliases = ["opus", "ds-flash", "ds-pro", "glm"].map(String::from);
        assert_eq!(complete_alias("o", &aliases), ("opus".into(), vec!["opus"]));
        assert_eq!(
            complete_alias("d", &aliases),
            ("ds-".into(), vec!["ds-flash", "ds-pro"])
        );
        assert_eq!(complete_alias("", &aliases).1.len(), 4);
        assert_eq!(complete_alias("x", &aliases), ("x".into(), vec![]));
    }

    #[test]
    fn tab_completion_only_applies_to_the_model_argument() {
        let mut app = App::for_test();
        app.input = "/model x".into();
        assert!(app.complete_model_alias());
        assert_eq!(app.input, "/model x");
        assert_eq!(
            app.status.as_deref(),
            Some("No model alias starts with 'x'")
        );

        for input in ["/model", "/clear x", "hello", "/model a b"] {
            app.input = input.into();
            assert!(!app.complete_model_alias(), "{input}");
        }
    }

    #[test]
    fn a_number_selects_and_confirms_that_model() {
        let mut app = app_with_picker();
        app.handle_model_picker_key(KeyCode::Char('3'));
        assert_eq!(app.model_picker.as_ref().unwrap().selected, 2);
        assert_eq!(
            app.status.as_deref(),
            Some("ds-flash is unavailable: DEEPSEEK_API_KEY is not set")
        );

        app.handle_model_picker_key(KeyCode::Char('9'));
        assert_eq!(app.model_picker.as_ref().unwrap().selected, 2);

        // No agent in tests: confirming an available model reports that.
        app.handle_model_picker_key(KeyCode::Char('1'));
        assert!(app.model_picker.is_none());
        assert_eq!(app.status.as_deref(), Some("Request in progress"));
    }

    #[test]
    fn escape_closes_without_switching() {
        let mut app = app_with_picker();
        app.handle_model_picker_key(KeyCode::Esc);
        assert!(app.model_picker.is_none());
        assert!(app.messages.is_empty());
        assert_eq!(app.model_label, "test");
    }
}
