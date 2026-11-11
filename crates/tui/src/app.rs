use crate::agent_task::{PendingSubmission, PendingTask};
use crate::input_history;
use crate::transcript::{
    ChatEntry, CommandOutputState, Speaker, transcript_entries_to_chat_entries,
};
use crate::view::text_layout::{help_text, truncate_text};
use color_eyre::{Result, eyre::eyre};
use ratatui::{
    DefaultTerminal,
    crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
};
use socium_core::constants::DEFAULT_MODEL;
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
    pub(crate) activity_log: Vec<String>,
    pub(crate) last_command_output: Option<CommandOutputState>,
    pub(crate) transcript_scroll: usize,
    pub(crate) status: Option<String>,
    pub(crate) workspace_label: String,
    pub(crate) model_label: String,
    pub(crate) context_metrics: ContextMetrics,
    pub(crate) last_latency_ms: Option<u128>,
    pub(crate) pending_task: Option<PendingTask>,
    pub(crate) input_history: input_history::InputSession,
    pub(crate) history_index: Option<usize>,
    pub(crate) history_draft: String,
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
        let model_label = env::var("MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
        let workspace_label = format_workspace_path(env::current_dir().ok());
        let messages = transcript_entries_to_chat_entries(agent.transcript_entries());
        let context_metrics = agent.context_metrics("").unwrap_or_default();
        let history_path =
            socium_core::workspace_state_path(&env::current_dir()?, INPUT_HISTORY_FILE_NAME)?;
        let input_history = input_history::InputSession::load(history_path)?;

        Ok(Self {
            agent: Some(agent),
            input: String::new(),
            character_index: 0,
            input_mode: InputMode::Editing,
            messages,
            activity_log: Vec::new(),
            last_command_output: None,
            transcript_scroll: 0,
            status: Some("Ready".to_string()),
            workspace_label,
            model_label,
            context_metrics,
            last_latency_ms: None,
            pending_task: None,
            input_history,
            history_index: None,
            history_draft: String::new(),
        })
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

        match user_input.as_str() {
            "/quit" | "/exit" => return Ok(SubmitAction::Exit),
            "/help" => {
                self.push_message(Speaker::System, help_text());
                self.status = Some("Help".to_string());
                return Ok(SubmitAction::None);
            }
            "/clear" => {
                self.agent_mut()?.clear()?;
                self.messages.clear();
                self.activity_log.clear();
                self.last_command_output = None;
                self.last_latency_ms = None;
                self.status = Some("Conversation cleared".to_string());
                self.refresh_context_metrics();
                return Ok(SubmitAction::None);
            }
            "/compact" => {
                self.push_message(Speaker::System, "Compacting conversation context…");
                self.status = Some("Compacting".to_string());
                return Ok(SubmitAction::Compact);
            }
            _ => {}
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
                        task.approval_id = None;
                        self.status =
                            Some("Cancelling; waiting for cleanup and saved results…".into());
                        continue;
                    }
                    if let Some(id) = task.approval_id {
                        if matches!(key.code, KeyCode::Char('y') | KeyCode::Char('n')) {
                            task.control
                                .answer_approval(id, key.code == KeyCode::Char('y'));
                            task.approval_id = None;
                            self.status = Some("Approval answered".into());
                        }
                        continue;
                    }
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
