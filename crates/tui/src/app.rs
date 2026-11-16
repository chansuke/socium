use crate::activity::ActivityModel;
use crate::agent_task::{PendingSubmission, PendingTask};
use crate::approval::ApprovalPrompt;
use crate::input_history;
use crate::model_picker::{ModelPicker, model_label};
use crate::rewind_picker::RewindPicker;
use crate::session_picker::SessionPicker;
use crate::slash_command::{self, SlashAction, SlashCommand};
use crate::transcript::{ChatEntry, Speaker, transcript_entries_to_chat_entries};
use crate::view::text_layout::{help_text, truncate_text};
use color_eyre::{Result, eyre::eyre};
use ratatui::{
    DefaultTerminal,
    crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
};
use socium_core::{AgentService, ContextMetrics, providers::TokenUsage};
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
    /// `approval.commands = "auto"`: shown in the footer.
    pub(crate) commands_run_without_asking: bool,
    pub(crate) context_metrics: ContextMetrics,
    /// Tokens this conversation used, and their cost when priced.
    pub(crate) session_usage: (TokenUsage, Option<f64>),
    pub(crate) last_latency_ms: Option<u128>,
    pub(crate) pending_task: Option<PendingTask>,
    pub(crate) approval: Option<ApprovalPrompt>,
    pub(crate) model_picker: Option<ModelPicker>,
    pub(crate) session_picker: Option<SessionPicker>,
    pub(crate) rewind_picker: Option<RewindPicker>,
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
        let warnings = agent.startup_warnings();
        let commands_run_without_asking = agent.commands_run_without_asking();
        let history_path =
            socium_core::workspace_state_path(&env::current_dir()?, INPUT_HISTORY_FILE_NAME)?;
        let input_history = input_history::InputSession::load(history_path)?;

        let mut app = Self {
            agent: Some(agent),
            input: String::new(),
            character_index: 0,
            input_mode: InputMode::Editing,
            messages: Vec::new(),
            activity: ActivityModel::default(),
            transcript_scroll: 0,
            status: None,
            workspace_label,
            model_label,
            commands_run_without_asking,
            context_metrics: ContextMetrics::default(),
            session_usage: Default::default(),
            last_latency_ms: None,
            pending_task: None,
            approval: None,
            model_picker: None,
            session_picker: None,
            rewind_picker: None,
            input_history,
            history_index: None,
            history_draft: String::new(),
            slash_selection: 0,
            slash_popup_dismissed: false,
        };
        app.load_session_view();
        for body in warnings {
            app.push_message(Speaker::System, body);
        }
        Ok(app)
    }

    /// Shows the agent's session: its conversation, a note when it was
    /// resumed, and its recent Activity.
    pub(crate) fn load_session_view(&mut self) {
        self.show_agent_conversation();
        if let Some(agent) = self.agent.as_ref()
            && agent.has_saved_messages()
        {
            let note = format!("Resumed session {}", agent.session_id());
            self.push_message(Speaker::System, note);
        }
    }

    /// Shows the agent's conversation and its recent Activity.
    pub(crate) fn show_agent_conversation(&mut self) {
        let Some(agent) = self.agent.as_ref() else {
            return;
        };
        let mut activity = ActivityModel::default();
        for event in agent.recent_activity() {
            activity.apply_replayed(event);
        }
        self.activity = activity;
        self.show_agent_transcript();
    }

    /// Shows the agent's conversation, leaving the Activity as it is.
    pub(crate) fn show_agent_transcript(&mut self) {
        let Some(agent) = self.agent.as_ref() else {
            return;
        };
        self.messages = transcript_entries_to_chat_entries(agent.transcript_entries());
        self.transcript_scroll = 0;
        self.last_latency_ms = None;
        self.refresh_context_metrics();
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
            commands_run_without_asking: false,
            context_metrics: Default::default(),
            session_usage: Default::default(),
            last_latency_ms: None,
            pending_task: None,
            approval: None,
            model_picker: None,
            session_picker: None,
            rewind_picker: None,
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

    /// Inserts pasted text at the cursor, newlines included, with CRLF and
    /// lone CR turned into LF.
    pub(crate) fn insert_text(&mut self, text: &str) {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        if self.history_index.is_some() {
            self.history_index = None;
            self.history_draft.clear();
        }
        let index = self.byte_index();
        self.input.insert_str(index, &text);
        self.character_index += text.chars().count();
        self.reset_slash_popup();
        self.refresh_context_metrics();
    }

    /// Enter ends the message unless it asks for a new line: with Alt or
    /// Shift, or after a backslash, which works in every terminal.
    fn enter_wants_newline(&self, modifiers: KeyModifiers) -> bool {
        modifiers.intersects(KeyModifiers::ALT | KeyModifiers::SHIFT)
            || self
                .character_index
                .checked_sub(1)
                .and_then(|before| self.input.chars().nth(before))
                == Some('\\')
    }

    fn insert_newline(&mut self) {
        if self.character_index > 0
            && self.input.chars().nth(self.character_index - 1) == Some('\\')
        {
            self.delete_char();
        }
        self.enter_char('\n');
    }

    /// Moves the cursor to the same column of the line above (`-1`) or
    /// below (`1`), as far as that line reaches.
    fn move_cursor_vertically(&mut self, step: isize) {
        let chars = self.input.chars().collect::<Vec<_>>();
        let line_start = |at: usize| {
            chars[..at]
                .iter()
                .rposition(|c| *c == '\n')
                .map_or(0, |newline| newline + 1)
        };
        let start = line_start(self.character_index);
        let column = self.character_index - start;
        let target_start = if step < 0 {
            if start == 0 {
                return;
            }
            line_start(start - 1)
        } else {
            match chars[self.character_index..]
                .iter()
                .position(|c| *c == '\n')
            {
                Some(offset) => self.character_index + offset + 1,
                None => return,
            }
        };
        let target_end = chars[target_start..]
            .iter()
            .position(|c| *c == '\n')
            .map_or(chars.len(), |offset| target_start + offset);
        self.character_index = (target_start + column).min(target_end);
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
            self.status = Some(
                "Request in progress; send this message when it ends, or Esc to cancel it"
                    .to_string(),
            );
            return Ok(SubmitAction::None);
        }
        let user_input = self.input.trim().to_string();
        if let Err(err) = self.input_history.record_entry(&user_input) {
            // Printing would draw over the TUI.
            self.push_message(
                Speaker::System,
                format!("Warning: failed to persist input history: {err}"),
            );
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
                let agent = self.agent_mut()?;
                let previous = agent
                    .has_saved_messages()
                    .then(|| agent.session_id().to_string());
                agent.clear()?;
                self.messages.clear();
                self.activity.clear();
                self.last_latency_ms = None;
                self.status = Some(match previous {
                    Some(id) => format!("Conversation cleared; resume it with -r {id}"),
                    None => "Conversation cleared".to_string(),
                });
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
            SlashAction::Resume => match argument {
                Some(query) => {
                    if self.resume_session(&query) {
                        return Ok(SubmitAction::Compact);
                    }
                }
                None => self.open_session_picker(),
            },
            SlashAction::Rename => self.rename_session(argument.as_deref()),
            SlashAction::Rewind => self.open_rewind_picker(),
            SlashAction::Agents => {
                let agent = self.agent_mut()?;
                let text = match argument {
                    None => agent.subagent_runs().map(|runs| {
                        let now_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map_or(0, |elapsed| elapsed.as_millis() as u64);
                        crate::subagents::runs_text(&runs, now_ms)
                    }),
                    Some(id) => agent.subagent_transcript(&id).map(|(summary, entries)| {
                        crate::subagents::transcript_text(&summary, &entries)
                    }),
                };
                self.push_message(
                    Speaker::System,
                    text.unwrap_or_else(|err| format!("Cannot read saved subagents: {err}")),
                );
                self.status = Some("Subagents".to_string());
            }
            SlashAction::Undo => {
                let message = match self.agent_mut()?.undo(argument.as_deref()) {
                    Ok(paths) => format!("Undid the last edit of {}", paths.join(", ")),
                    Err(err) => format!("Cannot undo: {err}"),
                };
                self.push_message(Speaker::System, message);
                self.status = Some("Undo".to_string());
                self.refresh_context_metrics();
            }
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

    /// Runs until the user quits, and returns the ID that resumes the
    /// conversation when it has been saved.
    pub(crate) async fn run(mut self, mut terminal: DefaultTerminal) -> Result<Option<String>> {
        self.run_loop(&mut terminal).await?;
        Ok(self
            .agent
            .as_ref()
            .filter(|agent| agent.has_saved_messages())
            .map(|agent| agent.session_id().to_string()))
    }

    async fn run_loop(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        loop {
            if let Some(message) = self.agent.as_mut().and_then(AgentService::sandbox_failure) {
                return Err(eyre!(message));
            }
            self.poll_pending_task();
            terminal.draw(|frame| self.draw(frame))?;

            if !event::poll(Duration::from_millis(50))? {
                continue;
            }

            let event = event::read()?;
            if let Event::Paste(text) = &event {
                if matches!(self.input_mode, InputMode::Editing)
                    && self.approval.is_none()
                    && self.model_picker.is_none()
                    && self.session_picker.is_none()
                    && self.rewind_picker.is_none()
                {
                    self.insert_text(text);
                }
                continue;
            }
            if let Event::Key(key) = event {
                if key.kind == KeyEventKind::Press && self.session_picker.is_some() {
                    if self.handle_session_picker_key(key.code, key.modifiers) {
                        terminal.draw(|frame| self.draw(frame))?;
                        self.start_compact_task()?;
                    }
                    continue;
                }
                if key.kind == KeyEventKind::Press && self.rewind_picker.is_some() {
                    self.handle_rewind_picker_key(key.code, key.modifiers);
                    continue;
                }
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
                        _ => {}
                    },
                    InputMode::Editing if key.kind == KeyEventKind::Press => match key.code {
                        KeyCode::Tab => {
                            self.complete_model_alias();
                        }
                        KeyCode::Enter if self.enter_wants_newline(key.modifiers) => {
                            self.insert_newline();
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
                        // In a message of several lines the arrows move
                        // between them; otherwise they recall history.
                        KeyCode::Up if self.input.contains('\n') => self.move_cursor_vertically(-1),
                        KeyCode::Down if self.input.contains('\n') => {
                            self.move_cursor_vertically(1)
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
                        // The next message can be written while a request
                        // runs; Enter sends it once the request is done.
                        KeyCode::Char(to_insert)
                            if !key.modifiers.contains(KeyModifiers::CONTROL)
                                && !key.modifiers.contains(KeyModifiers::ALT) =>
                        {
                            if self.history_index.is_some() {
                                self.history_index = None;
                                self.history_draft.clear();
                            }
                            self.enter_char(to_insert);
                        }
                        KeyCode::Backspace => {
                            if self.history_index.is_some() {
                                self.history_index = None;
                                self.history_draft.clear();
                            }
                            self.delete_char();
                        }
                        KeyCode::Left => self.move_cursor_left(),
                        KeyCode::Right => self.move_cursor_right(),
                        KeyCode::Esc => {
                            self.input_mode = InputMode::Normal;
                            self.status = Some(if self.pending_task.is_some() {
                                "Request in progress".to_string()
                            } else {
                                "Press E to resume typing".to_string()
                            });
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
        if let Some(agent) = self.agent.as_ref() {
            self.session_usage = agent.session_usage();
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
    fn a_paste_keeps_every_line_with_unix_line_endings() {
        // Review N13: only the first pasted line was sent.
        let mut app = App::for_test();
        type_text(&mut app, "> ");
        app.insert_text("first\r\nsecond\rthird");
        assert_eq!(app.input, "> first\nsecond\nthird");
        assert_eq!(app.character_index, app.input.chars().count());
    }

    #[test]
    fn enter_after_a_backslash_or_with_alt_adds_a_line() {
        let mut app = App::for_test();
        type_text(&mut app, "line one\\");
        assert!(app.enter_wants_newline(KeyModifiers::NONE));
        app.insert_newline();
        assert_eq!(app.input, "line one\n");
        type_text(&mut app, "two");
        assert!(!app.enter_wants_newline(KeyModifiers::NONE));
        assert!(app.enter_wants_newline(KeyModifiers::ALT));
        assert!(app.enter_wants_newline(KeyModifiers::SHIFT));
    }

    #[test]
    fn arrows_move_between_the_lines_of_a_message() {
        let mut app = App::for_test();
        app.insert_text("abcd\nef");
        app.move_cursor_vertically(-1);
        assert_eq!(app.character_index, 2);
        app.move_cursor_vertically(-1);
        assert_eq!(app.character_index, 2);
        app.character_index = 4;
        app.move_cursor_vertically(1);
        assert_eq!(app.character_index, 7, "clamped to the shorter line");
        app.move_cursor_vertically(1);
        assert_eq!(app.character_index, 7);
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
