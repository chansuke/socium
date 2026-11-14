use crate::activity::ActivityModel;
use crate::agent_task::{PendingSubmission, PendingTask};
use crate::approval::ApprovalPrompt;
use crate::input_history;
use crate::model_picker::{ModelPicker, model_label};
use crate::slash_command::{self, SlashAction, SlashCommand};
use crate::transcript::{ChatEntry, Speaker, transcript_entries_to_chat_entries};
use crate::view::text_layout::{help_text, truncate_text};
use color_eyre::{Result, eyre::eyre};
use ratatui::{
    DefaultTerminal,
    crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
};
use socium_core::{AgentService, ContextMetrics};
use std::{
    env,
    path::PathBuf,
    time::{Duration, Instant},
};

const INPUT_HISTORY_FILE_NAME: &str = "input_history.json";

pub(crate) struct App {
    pub(crate) agent: Option<AgentService>,
    pub(crate) input: String,
    pub(crate) character_index: usize,
    pub(crate) input_mode: InputMode,
    pub(crate) messages: Vec<ChatEntry>,
    pub(crate) activity: ActivityModel,
    pub(crate) transcript_scroll: usize,
    pub(crate) status: Option<String>,
    pub(crate) workspace_label: String,
    pub(crate) model_label: String,
    pub(crate) context_metrics: ContextMetrics,
    pub(crate) last_latency_ms: Option<u128>,
    pub(crate) pending_task: Option<PendingTask>,
    pub(crate) approval: Option<ApprovalPrompt>,
    pub(crate) model_picker: Option<ModelPicker>,
    pub(crate) input_history: input_history::InputSession,
    pub(crate) history_index: Option<usize>,
    pub(crate) history_draft: String,
    pub(crate) slash_selection: usize,
    slash_popup_dismissed: bool,
}

#[derive(Clone, Copy)]
pub(crate) enum InputMode {
    Normal,
    Editing,
}

enum SubmitAction {
    None,
    Exit,
    Pending(PendingSubmission),
    Compact,
}

impl App {
    pub(crate) fn new(agent: AgentService) -> Result<Self> {
        let model_label = model_label(&agent.active_model());
        let workspace_label = format_workspace_path(env::current_dir().ok());
        let mut messages = transcript_entries_to_chat_entries(agent.transcript_entries());
        messages.extend(agent.startup_warnings().into_iter().map(|body| ChatEntry {
            speaker: Speaker::System,
            body,
        }));
        let context_metrics = agent.context_metrics("").unwrap_or_default();
        let mut activity = ActivityModel::default();
        for event in agent.recent_activity() {
            activity.apply_replayed(event);
        }
        let history_path =
            socium_core::workspace_state_path(&env::current_dir()?, INPUT_HISTORY_FILE_NAME)?;
        let input_history = input_history::InputSession::load(history_path)?;

        Ok(Self {
            agent: Some(agent),
            input: String::new(),
            character_index: 0,
            input_mode: InputMode::Editing,
            messages,
            activity,
            transcript_scroll: 0,
            status: Some(socium_core::SANDBOX_DESCRIPTION.to_string()),
            workspace_label,
            model_label,
            context_metrics,
            last_latency_ms: None,
            pending_task: None,
            approval: None,
            model_picker: None,
            input_history,
            history_index: None,
            history_draft: String::new(),
            slash_selection: 0,
            slash_popup_dismissed: false,
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self {
            agent: None,
            input: String::new(),
            character_index: 0,
            input_mode: InputMode::Editing,
            messages: Vec::new(),
            activity: ActivityModel::default(),
            transcript_scroll: 0,
            status: None,
            workspace_label: "test".into(),
            model_label: "test".into(),
            context_metrics: Default::default(),
            last_latency_ms: None,
            pending_task: None,
            approval: None,
            model_picker: None,
            input_history: input_history::InputSession {
                history_path: PathBuf::new(),
                entries: Vec::new(),
            },
            history_index: None,
            history_draft: String::new(),
            slash_selection: 0,
            slash_popup_dismissed: false,
        }
    }

    fn move_cursor_left(&mut self) {
        self.character_index = self.clamp_cursor(self.character_index.saturating_sub(1));
    }

    fn move_cursor_right(&mut self) {
        self.character_index = self.clamp_cursor(self.character_index.saturating_add(1));
    }

    fn enter_char(&mut self, new_char: char) {
        let index = self.byte_index();
        self.input.insert(index, new_char);
        self.move_cursor_right();
        self.reset_slash_popup();
        self.refresh_context_metrics();
    }

    fn byte_index(&self) -> usize {
        self.input
            .char_indices()
            .map(|(i, _)| i)
            .nth(self.character_index)
            .unwrap_or(self.input.len())
    }

    fn delete_char(&mut self) {
        if self.character_index == 0 {
            return;
        }

        let current_index = self.character_index;
        let before_char_to_delete = self.input.chars().take(current_index - 1);
        let after_char_to_delete = self.input.chars().skip(current_index);
        self.input = before_char_to_delete.chain(after_char_to_delete).collect();
        self.move_cursor_left();
        self.reset_slash_popup();
        self.refresh_context_metrics();
    }

    fn clamp_cursor(&self, new_cursor_pos: usize) -> usize {
        new_cursor_pos.clamp(0, self.input.chars().count())
    }

    fn reset_cursor(&mut self) {
        self.character_index = 0;
    }

    fn history_prev(&mut self) {
        if self.input_history.entries.is_empty() {
            return;
        }
        match self.history_index {
            None => {
                self.history_draft = self.input.clone();
                self.history_index = Some(self.input_history.entries.len() - 1);
            }
            Some(i) if i > 0 => {
                self.history_index = Some(i - 1);
            }
            _ => {}
        }
        if let Some(i) = self.history_index {
            self.input = self.input_history.entries[i].clone();
            self.character_index = self.input.chars().count();
            self.refresh_context_metrics();
        }
    }

    fn history_next(&mut self) {
        if let Some(i) = self.history_index {
            if i + 1 < self.input_history.entries.len() {
                self.history_index = Some(i + 1);
                self.input = self.input_history.entries[i + 1].clone();
            } else {
                self.history_index = None;
                self.input = self.history_draft.clone();
            }
            self.character_index = self.input.chars().count();
            self.refresh_context_metrics();
        }
    }

    fn reset_slash_popup(&mut self) {
        self.slash_selection = 0;
        self.slash_popup_dismissed = false;
    }

    /// Commands offered while a bare `/name` is being typed. Recalled history
    /// entries keep Up/Down for history navigation, so they show no popup.
    pub(crate) fn slash_suggestions(&self) -> Vec<&'static SlashCommand> {
        if self.slash_popup_dismissed
            || self.pending_task.is_some()
            || self.history_index.is_some()
            || !matches!(self.input_mode, InputMode::Editing)
        {
            return Vec::new();
        }
        slash_command::matching_commands(&self.input)
    }

    fn move_slash_selection(&mut self, len: usize, forward: bool) {
        self.slash_selection = if forward {
            (self.slash_selection + 1) % len
        } else {
            (self.slash_selection + len - 1) % len
        };
    }

    fn complete_slash_command(&mut self, command: &SlashCommand) {
        self.input = format!("/{}", command.name);
        self.character_index = self.input.chars().count();
        self.slash_selection = 0;
        self.refresh_context_metrics();
    }

    /// Handles keys owned by the open completion popup. Returns `true` when the
    /// key was consumed; Enter only completes here and is submitted by the caller.
    fn handle_slash_popup_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        let suggestions = self.slash_suggestions();
        if suggestions.is_empty() {
            return false;
        }
        let selected = suggestions[self.slash_selection.min(suggestions.len() - 1)];
        let control = modifiers.contains(KeyModifiers::CONTROL);
        match code {
            KeyCode::Up => self.move_slash_selection(suggestions.len(), false),
            KeyCode::Char('p') if control => self.move_slash_selection(suggestions.len(), false),
            KeyCode::Down => self.move_slash_selection(suggestions.len(), true),
            KeyCode::Char('n') if control => self.move_slash_selection(suggestions.len(), true),
            KeyCode::Tab => self.complete_slash_command(selected),
            KeyCode::Esc => self.slash_popup_dismissed = true,
            KeyCode::Enter => {
                self.complete_slash_command(selected);
                return false;
            }
            _ => return false,
        }
        true
    }

    /// Keys owned by the Activity pane. Ctrl+A toggles focus from anywhere;
    /// while focused, navigation keys are consumed here and Esc returns to
    /// the input instead of cancelling. Cancel (Ctrl+C) and approval (y / n)
    /// still reach the task handling.
    fn handle_activity_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        // An open approval owns the screen and the keys until it is answered.
        if self.approval.is_some() {
            return false;
        }
        let control = modifiers.contains(KeyModifiers::CONTROL);
        if control && code == KeyCode::Char('a') {
            self.activity.toggle_focus();
            self.status = Some(if self.activity.focused {
                "Activity: Up/Down select · Ctrl+L top · Enter expand · r raw · z full screen · Esc back"
                    .to_string()
            } else {
                "Ready".to_string()
            });
            return true;
        }
        if !self.activity.focused {
            return false;
        }
        if let Some(view) = self.activity.raw_view.as_mut() {
            match code {
                KeyCode::Esc | KeyCode::Char('r') => self.activity.raw_view = None,
                KeyCode::Up | KeyCode::Char('k') => view.scroll += 1,
                KeyCode::Down | KeyCode::Char('j') => view.scroll = view.scroll.saturating_sub(1),
                KeyCode::PageUp => view.scroll += 10,
                KeyCode::PageDown => view.scroll = view.scroll.saturating_sub(10),
                KeyCode::Char('c') if control => return false,
                KeyCode::Char('y' | 'n' | '1' | '2') if self.approval.is_some() => return false,
                _ => {}
            }
            return true;
        }
        match code {
            KeyCode::Esc => {
                self.activity.toggle_focus();
                self.status = Some("Ready".to_string());
            }
            KeyCode::Up | KeyCode::Char('k') => self.activity.select_by(-1),
            KeyCode::Down | KeyCode::Char('j') => self.activity.select_by(1),
            KeyCode::PageUp => self.activity.select_by(-10),
            KeyCode::PageDown => self.activity.select_by(10),
            KeyCode::Char('l') if control => self.activity.select_first(),
            KeyCode::Home | KeyCode::Char('g') => self.activity.select_first(),
            KeyCode::End | KeyCode::Char('G') => self.activity.select_last(),
            KeyCode::Enter => self.activity.toggle_expanded(),
            KeyCode::Char('z') => self.activity.fullscreen = !self.activity.fullscreen,
            KeyCode::Char('r') => {
                if let Err(reason) = self.activity.open_raw_view() {
                    self.status = Some(reason.to_string());
                }
            }
            KeyCode::Char('c') if control => return false,
            KeyCode::Char('y' | 'n' | '1' | '2') if self.approval.is_some() => return false,
            _ => {}
        }
        true
    }

    fn begin_submit(&mut self) -> Result<SubmitAction> {
        if self.pending_task.is_some() {
            self.status = Some("Request already in progress".to_string());
            return Ok(SubmitAction::None);
        }
        let user_input = self.input.trim().to_string();
        if let Err(err) = self.input_history.record_entry(&user_input) {
            eprintln!("Warning: failed to persist input history: {err}");
        }
        self.history_index = None;
        self.history_draft.clear();
        self.input.clear();
        self.reset_cursor();

        if user_input.is_empty() {
            self.status = Some("Input cannot be empty".to_string());
            return Ok(SubmitAction::None);
        }

        if let Some((command, argument)) = slash_command::find(&user_input) {
            let argument = argument.map(str::to_string);
            return self.run_slash_command(command.action, argument);
        }
        if slash_command::command_token(&user_input).is_some() {
            self.push_message(
                Speaker::System,
                format!("Unknown command: {user_input}. Type /commands to list commands."),
            );
            self.status = Some("Unknown command".to_string());
            return Ok(SubmitAction::None);
        }

        self.push_message(Speaker::User, user_input.clone());
        self.push_message(
            Speaker::Thinking,
            format!(
                "Preparing response for: \"{}\"",
                truncate_text(&user_input, 64)
            ),
        );
        self.status = Some("Thinking".to_string());

        Ok(SubmitAction::Pending(PendingSubmission {
            user_input,
            started_at: Instant::now(),
        }))
    }

    fn run_slash_command(
        &mut self,
        action: SlashAction,
        argument: Option<String>,
    ) -> Result<SubmitAction> {
        match action {
            SlashAction::Exit => return Ok(SubmitAction::Exit),
            SlashAction::Help => {
                self.push_message(Speaker::System, help_text());
                self.status = Some("Help".to_string());
            }
            SlashAction::Commands => {
                self.push_message(Speaker::System, slash_command::command_list_text());
                self.status = Some("Commands".to_string());
            }
            SlashAction::Clear => {
                self.agent_mut()?.clear()?;
                self.messages.clear();
                self.activity.clear();
                self.last_latency_ms = None;
                self.status = Some("Conversation cleared".to_string());
                self.refresh_context_metrics();
            }
            SlashAction::Model => match argument {
                Some(alias) => {
                    if self.switch_model(&alias) {
                        return Ok(SubmitAction::Compact);
                    }
                }
                None => {
                    let choices = self.agent_mut()?.available_models();
                    self.open_model_picker(choices);
                }
            },
            SlashAction::Compact => {
                self.push_message(Speaker::System, "Compacting conversation context…");
                self.status = Some("Compacting".to_string());
                return Ok(SubmitAction::Compact);
            }
        }
        Ok(SubmitAction::None)
    }

    fn agent_mut(&mut self) -> Result<&mut AgentService> {
        self.agent
            .as_mut()
            .ok_or_else(|| eyre!("agent is unavailable while a request is running"))
    }

    pub(crate) async fn run(mut self, mut terminal: DefaultTerminal) -> Result<()> {
        loop {
            self.poll_pending_task();
            terminal.draw(|frame| self.draw(frame))?;

            if !event::poll(Duration::from_millis(50))? {
                continue;
            }

            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press && self.model_picker.is_some() {
                    if self.handle_model_picker_key(key.code) {
                        terminal.draw(|frame| self.draw(frame))?;
                        self.start_compact_task()?;
                    }
                    continue;
                }
                if key.kind == KeyEventKind::Press
                    && self.handle_activity_key(key.code, key.modifiers)
                {
                    continue;
                }
                if key.kind == KeyEventKind::Press
                    && let Some(task) = self.pending_task.as_mut()
                {
                    if key.code == KeyCode::Esc
                        || (key.code == KeyCode::Char('c')
                            && key.modifiers.contains(KeyModifiers::CONTROL))
                        || (matches!(self.input_mode, InputMode::Normal)
                            && key.code == KeyCode::Char('q'))
                    {
                        task.control.cancel();
                        self.approval = None;
                        self.status =
                            Some("Cancelling; waiting for cleanup and saved results…".into());
                        continue;
                    }
                    if self.approval.is_some() {
                        self.handle_approval_key(key.code);
                        continue;
                    }
                }
                if key.kind == KeyEventKind::Press
                    && matches!(self.input_mode, InputMode::Editing)
                    && self.handle_slash_popup_key(key.code, key.modifiers)
                {
                    continue;
                }
                match self.input_mode {
                    InputMode::Normal if key.kind == KeyEventKind::Press => match key.code {
                        KeyCode::Char('e') | KeyCode::Enter => {
                            self.input_mode = InputMode::Editing;
                            if self.pending_task.is_none() {
                                self.status = Some("Ready".to_string());
                            }
                        }
                        KeyCode::Up | KeyCode::Char('k') => self.scroll_transcript_up(1),
                        KeyCode::Down | KeyCode::Char('j') => self.scroll_transcript_down(1),
                        KeyCode::PageUp => self.scroll_transcript_up(10),
                        KeyCode::PageDown => self.scroll_transcript_down(10),
                        KeyCode::Char('q') => return Ok(()),
                        _ => {}
                    },
                    InputMode::Editing if key.kind == KeyEventKind::Press => match key.code {
                        KeyCode::Tab => {
                            self.complete_model_alias();
                        }
                        KeyCode::Enter => match self.begin_submit()? {
                            SubmitAction::None => {}
                            SubmitAction::Exit => return Ok(()),
                            SubmitAction::Pending(pending) => {
                                terminal.draw(|frame| self.draw(frame))?;
                                self.start_submit_task(pending)?;
                            }
                            SubmitAction::Compact => {
                                terminal.draw(|frame| self.draw(frame))?;
                                self.start_compact_task()?;
                            }
                        },
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            return Ok(());
                        }
                        KeyCode::Up if self.pending_task.is_none() => self.history_prev(),
                        KeyCode::Down if self.pending_task.is_none() => self.history_next(),
                        KeyCode::Char('p')
                            if self.pending_task.is_none()
                                && key.modifiers.contains(KeyModifiers::CONTROL) =>
                        {
                            self.history_prev();
                        }
                        KeyCode::Char('n')
                            if self.pending_task.is_none()
                                && key.modifiers.contains(KeyModifiers::CONTROL) =>
                        {
                            self.history_next();
                        }
                        KeyCode::Char(to_insert)
                            if self.pending_task.is_none()
                                && !key.modifiers.contains(KeyModifiers::CONTROL)
                                && !key.modifiers.contains(KeyModifiers::ALT) =>
                        {
                            if self.history_index.is_some() {
                                self.history_index = None;
                                self.history_draft.clear();
                            }
                            self.enter_char(to_insert);
                        }
                        KeyCode::Backspace if self.pending_task.is_none() => {
                            if self.history_index.is_some() {
                                self.history_index = None;
                                self.history_draft.clear();
                            }
                            self.delete_char();
                        }
                        KeyCode::Left if self.pending_task.is_none() => self.move_cursor_left(),
                        KeyCode::Right if self.pending_task.is_none() => self.move_cursor_right(),
                        KeyCode::Esc => {
                            self.input_mode = InputMode::Normal;
                            self.status = Some(if self.pending_task.is_some() {
                                "Request in progress".to_string()
                            } else {
                                "Press E to resume typing".to_string()
                            });
                        }
                        KeyCode::Char(_)
                            if !key.modifiers.contains(KeyModifiers::CONTROL)
                                && !key.modifiers.contains(KeyModifiers::ALT)
                                && self.pending_task.is_some() =>
                        {
                            self.status = Some("Request in progress".to_string());
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
        }
    }

    fn scroll_transcript_up(&mut self, amount: usize) {
        self.transcript_scroll = self.transcript_scroll.saturating_add(amount.max(1));
        self.status = Some("Transcript scroll".to_string());
    }

    fn scroll_transcript_down(&mut self, amount: usize) {
        self.transcript_scroll = self.transcript_scroll.saturating_sub(amount.max(1));
        self.status = Some(if self.transcript_scroll == 0 {
            "Ready".to_string()
        } else {
            "Transcript scroll".to_string()
        });
    }

    pub(crate) fn refresh_context_metrics(&mut self) {
        if let Some(agent) = self.agent.as_ref()
            && let Ok(metrics) = agent.context_metrics(&self.input)
        {
            self.context_metrics = metrics;
        }
    }
}

fn format_workspace_path(path: Option<PathBuf>) -> String {
    let Some(path) = path else {
        return "~".to_string();
    };

    let home = env::var_os("HOME").map(PathBuf::from);
    if let Some(home) = home
        && let Ok(stripped) = path.strip_prefix(&home)
    {
        let suffix = stripped.display().to_string();
        return if suffix.is_empty() {
            "~".to_string()
        } else {
            format!("~/{}", suffix)
        };
    }

    path.display().to_string()
}

impl Drop for App {
    fn drop(&mut self) {
        if let Some(mut task) = self.pending_task.take() {
            task.control.cancel();
            if let Some(worker) = task.worker.take() {
                let _ = worker.join();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn type_text(app: &mut App, text: &str) {
        for ch in text.chars() {
            app.enter_char(ch);
        }
    }

    fn suggestion_names(app: &App) -> Vec<&'static str> {
        app.slash_suggestions()
            .into_iter()
            .map(|command| command.name)
            .collect()
    }

    #[test]
    fn typing_a_slash_opens_suggestions_and_prefix_filters_them() {
        let mut app = App::for_test();
        type_text(&mut app, "/");
        assert_eq!(
            suggestion_names(&app).len(),
            slash_command::SLASH_COMMANDS.len()
        );

        type_text(&mut app, "co");
        assert_eq!(suggestion_names(&app), ["commands", "compact"]);

        app.delete_char();
        app.delete_char();
        type_text(&mut app, "x");
        assert!(suggestion_names(&app).is_empty());
    }

    #[test]
    fn arrows_wrap_and_tab_completes_the_selected_command() {
        let mut app = App::for_test();
        type_text(&mut app, "/co");

        assert!(app.handle_slash_popup_key(KeyCode::Down, KeyModifiers::NONE));
        assert!(app.handle_slash_popup_key(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.slash_selection, 0);
        assert!(app.handle_slash_popup_key(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.slash_selection, 1);

        assert!(app.handle_slash_popup_key(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.input, "/compact");
        assert_eq!(app.character_index, "/compact".len());
    }

    #[test]
    fn enter_runs_the_selected_command_instead_of_the_partial_input() {
        let mut app = App::for_test();
        type_text(&mut app, "/comm");

        assert!(!app.handle_slash_popup_key(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.input, "/commands");
        assert!(matches!(app.begin_submit().unwrap(), SubmitAction::None));
        assert_eq!(
            app.messages.last().unwrap().body,
            slash_command::command_list_text()
        );
    }

    #[test]
    fn esc_closes_the_popup_until_the_input_changes() {
        let mut app = App::for_test();
        type_text(&mut app, "/c");

        assert!(app.handle_slash_popup_key(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.slash_suggestions().is_empty());
        assert!(matches!(app.input_mode, InputMode::Editing));

        type_text(&mut app, "l");
        assert_eq!(suggestion_names(&app), ["clear"]);
    }

    #[test]
    fn recalled_history_keeps_arrow_keys_for_history() {
        let mut app = App::for_test();
        app.input_history.entries = vec!["/clear".into(), "/compact".into()];

        app.history_prev();
        assert_eq!(app.input, "/compact");
        assert!(app.slash_suggestions().is_empty());
        assert!(!app.handle_slash_popup_key(KeyCode::Up, KeyModifiers::NONE));
    }

    #[test]
    fn unknown_command_is_reported_instead_of_sent_to_the_model() {
        let mut app = App::for_test();
        type_text(&mut app, "/frobnicate");

        assert!(matches!(app.begin_submit().unwrap(), SubmitAction::None));
        let reply = &app.messages.last().unwrap().body;
        assert!(reply.contains("Unknown command: /frobnicate"));
        assert!(reply.contains("/commands"));
    }

    #[test]
    fn bare_exit_quits_but_longer_messages_do_not() {
        let mut app = App::for_test();
        type_text(&mut app, "exit");
        assert!(matches!(app.begin_submit().unwrap(), SubmitAction::Exit));

        type_text(&mut app, "exit the loop early");
        assert!(matches!(
            app.begin_submit().unwrap(),
            SubmitAction::Pending(_)
        ));
    }

    #[test]
    fn message_starting_with_a_path_is_still_sent_to_the_model() {
        let mut app = App::for_test();
        type_text(&mut app, "/usr/bin is missing rg");

        assert!(app.slash_suggestions().is_empty());
        assert!(matches!(
            app.begin_submit().unwrap(),
            SubmitAction::Pending(_)
        ));
    }

    #[test]
    fn ctrl_a_focuses_activity_and_esc_returns_without_cancelling() {
        let mut app = App::for_test();
        for event in crate::activity::tests::sample_turn() {
            app.activity.apply(event);
        }

        assert!(!app.handle_activity_key(KeyCode::Up, KeyModifiers::NONE));
        assert!(app.handle_activity_key(KeyCode::Char('a'), KeyModifiers::CONTROL));
        assert!(app.activity.focused);

        assert!(app.handle_activity_key(KeyCode::Up, KeyModifiers::NONE));
        assert!(app.activity.selected.is_some());
        assert!(app.handle_activity_key(KeyCode::Char('z'), KeyModifiers::NONE));
        assert!(app.activity.fullscreen);
        // Cancel still reaches the task handling while focused.
        assert!(!app.handle_activity_key(KeyCode::Char('c'), KeyModifiers::CONTROL));

        assert!(app.handle_activity_key(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.activity.focused);
        assert!(!app.activity.fullscreen);
    }

    #[test]
    fn ctrl_l_moves_the_activity_selection_to_the_top() {
        let mut app = App::for_test();
        for event in crate::activity::tests::sample_turn() {
            app.activity.apply(event);
        }

        assert!(!app.handle_activity_key(KeyCode::Char('l'), KeyModifiers::CONTROL));
        app.handle_activity_key(KeyCode::Char('a'), KeyModifiers::CONTROL);
        app.activity.select_last();

        assert!(app.handle_activity_key(KeyCode::Char('l'), KeyModifiers::CONTROL));
        assert_eq!(app.activity.selected, Some(0));
    }
}
