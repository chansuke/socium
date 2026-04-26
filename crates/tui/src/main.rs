#[allow(dead_code)]
mod repl;

use color_eyre::{Result, eyre::eyre};
use ratatui::{
    DefaultTerminal, Frame,
    crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    layout::{Constraint, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Paragraph},
};
use socium_core::{
    AgentProgress, AgentReply, AgentService, ContextMetrics, TranscriptEntry, TranscriptRole,
};
use std::{
    env,
    path::PathBuf,
    time::{Duration, Instant},
};
use tokio::sync::{
    mpsc::{UnboundedReceiver, unbounded_channel},
    oneshot,
};

const INPUT_HISTORY_FILE_NAME: &str = "input_history.json";

const BG: Color = Color::Rgb(10, 14, 24);
const TEXT: Color = Color::Rgb(220, 223, 235);
const MUTED: Color = Color::Rgb(118, 124, 151);
const BLUE: Color = Color::Rgb(102, 160, 255);
const LILAC: Color = Color::Rgb(187, 145, 255);
const PINK: Color = Color::Rgb(232, 126, 169);
const SOFT: Color = Color::Rgb(83, 91, 117);
const CODE_BORDER: Color = Color::Rgb(104, 94, 72);
const CODE_TEXT: Color = Color::Rgb(198, 204, 225);

const LOGO_WORDMARK: [&str; 6] = [
    "███████╗  ██████╗   ██████╗  ██╗  ██╗   ██╗  ███╗   ███╗",
    "██╔════╝ ██╔═══██╗ ██╔════╝  ██║  ██║   ██║  ████╗ ████║",
    "███████╗ ██║   ██║ ██║       ██║  ██║   ██║  ██╔████╔██║",
    "╚════██║ ██║   ██║ ██║       ██║  ██║   ██║  ██║╚██╔╝██║",
    "███████║ ╚██████╔╝ ╚██████╗  ██║  ╚██████╔╝  ██║ ╚═╝ ██║",
    "╚══════╝  ╚═════╝   ╚═════╝  ╚═╝   ╚═════╝   ╚═╝     ╚═╝",
];

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    let agent = AgentService::from_current_dir().map_err(|err| eyre!(err.to_string()))?;
    let terminal = ratatui::init();
    let app_result = App::new(agent).run(terminal).await;
    ratatui::restore();
    app_result
}

struct App {
    agent: Option<AgentService>,
    input: String,
    character_index: usize,
    input_mode: InputMode,
    messages: Vec<ChatEntry>,
    activity_log: Vec<String>,
    last_command_output: Option<CommandOutputState>,
    transcript_scroll: usize,
    status: Option<String>,
    workspace_label: String,
    model_label: String,
    context_metrics: ContextMetrics,
    last_latency_ms: Option<u128>,
    pending_task: Option<PendingTask>,
    input_history: repl::InputSession,
    history_index: Option<usize>,
    history_draft: String,
}

#[derive(Clone, Copy)]
enum InputMode {
    Normal,
    Editing,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Speaker {
    User,
    Socium,
    Thinking,
    System,
}

struct ChatEntry {
    speaker: Speaker,
    body: String,
}

struct CommandOutputState {
    command: String,
    exit_code: Option<i32>,
    timed_out: bool,
    stdout_preview: String,
    stderr_preview: String,
}

struct PendingSubmission {
    user_input: String,
    started_at: Instant,
}

struct PendingTask {
    progress_rx: UnboundedReceiver<AgentProgress>,
    result_rx: oneshot::Receiver<SubmitOutcome>,
    streamed_assistant_text: bool,
}

struct SubmitOutcome {
    agent: AgentService,
    reply: Result<AgentReply, String>,
    latency_ms: u128,
}

enum SubmitAction {
    None,
    Exit,
    Pending(PendingSubmission),
}

enum Segment {
    Text(String),
    Code { label: String, lines: Vec<String> },
}

impl App {
    fn new(agent: AgentService) -> Self {
        let model_label = env::var("MODEL").unwrap_or_else(|_| "socium-default".to_string());
        let workspace_label = format_workspace_path(env::current_dir().ok());
        let messages = transcript_entries_to_chat_entries(agent.transcript_entries());
        let context_metrics = agent.context_metrics("").unwrap_or_default();
        let history_path = env::current_dir()
            .unwrap_or_default()
            .join(".socium")
            .join(INPUT_HISTORY_FILE_NAME);
        let input_history =
            repl::InputSession::load(history_path.clone()).unwrap_or(repl::InputSession {
                history_path,
                entries: Vec::new(),
            });

        Self {
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

    fn push_message(&mut self, speaker: Speaker, body: impl Into<String>) {
        self.messages.push(ChatEntry {
            speaker,
            body: body.into(),
        });
        self.transcript_scroll = 0;
    }

    fn push_activity(&mut self, line: impl Into<String>) {
        self.activity_log.push(line.into());
    }

    fn append_streamed_assistant_text(&mut self, text: &str) {
        append_streamed_assistant_text(&mut self.messages, text);
        self.transcript_scroll = 0;
    }

    fn pop_thinking_message(&mut self) {
        if self
            .messages
            .last()
            .is_some_and(|entry| entry.speaker == Speaker::Thinking)
        {
            self.messages.pop();
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

    fn start_submit_task(&mut self, pending: PendingSubmission) -> Result<()> {
        let Some(mut agent) = self.agent.take() else {
            return Err(eyre!("agent is unavailable while a request is running"));
        };
        self.context_metrics = agent
            .context_metrics(&pending.user_input)
            .unwrap_or_default();
        let (progress_tx, progress_rx) = unbounded_channel();
        let (result_tx, result_rx) = oneshot::channel();
        let PendingSubmission {
            user_input,
            started_at,
        } = pending;

        tokio::spawn(async move {
            let reply = agent
                .submit_with_progress(user_input, |event| {
                    let _ = progress_tx.send(event);
                })
                .await
                .map_err(|err| err.to_string());
            let _ = result_tx.send(SubmitOutcome {
                agent,
                reply,
                latency_ms: started_at.elapsed().as_millis(),
            });
        });

        self.pending_task = Some(PendingTask {
            progress_rx,
            result_rx,
            streamed_assistant_text: false,
        });
        Ok(())
    }

    fn poll_pending_task(&mut self) {
        let mut progress_events = Vec::new();
        let mut streamed_assistant_text = None;
        let mut completed = None;

        if let Some(task) = self.pending_task.as_mut() {
            while let Ok(event) = task.progress_rx.try_recv() {
                if matches!(event, AgentProgress::AssistantText { .. }) {
                    task.streamed_assistant_text = true;
                }
                progress_events.push(event);
            }
            streamed_assistant_text = Some(task.streamed_assistant_text);
            if let Ok(outcome) = task.result_rx.try_recv() {
                completed = Some(outcome);
            }
        }

        for event in progress_events {
            self.apply_progress(event);
        }

        let Some(outcome) = completed else {
            return;
        };
        let streamed_assistant_text = streamed_assistant_text.unwrap_or(false);
        self.pending_task = None;
        self.agent = Some(outcome.agent);
        self.refresh_context_metrics();
        self.last_latency_ms = Some(outcome.latency_ms);
        self.pop_thinking_message();

        match outcome.reply {
            Ok(reply) => {
                if !streamed_assistant_text {
                    if reply.text.trim().is_empty() {
                        self.push_message(Speaker::Socium, "(no text response)");
                    } else {
                        self.push_message(Speaker::Socium, reply.text);
                    }
                }
                self.push_activity("Request completed");
                self.status = Some("Done".to_string());
            }
            Err(err) => {
                self.push_message(Speaker::System, format!("Request failed: {err}"));
                self.push_activity(format!("Request failed: {err}"));
                self.status = Some("Failed".to_string());
            }
        }
    }

    fn apply_progress(&mut self, event: AgentProgress) {
        match event {
            AgentProgress::ModelRequestStarted { round, provider } => {
                self.push_activity(format!("Model request round {round} via {provider}"));
                self.status = Some(model_request_status(round, &provider));
            }
            AgentProgress::ApprovalRequired { tool, reason } => {
                self.push_activity(format!("Approval required for {tool}: {reason}"));
                self.push_message(
                    Speaker::System,
                    format!("Approval required for {tool}: {reason}"),
                );
                self.status = Some("Waiting for approval".to_string());
            }
            AgentProgress::AssistantText { text } => {
                self.pop_thinking_message();
                if !text.is_empty() {
                    self.append_streamed_assistant_text(&text);
                    self.status = Some("Responding".to_string());
                }
            }
            AgentProgress::ToolCallStarted { name } => {
                self.push_activity(format!("Running tool: {name}"));
                self.pop_thinking_message();
                self.push_message(Speaker::System, format!("Running tool: {name}"));
                self.status = Some(format!("Tool: {name}"));
            }
            AgentProgress::ToolCallFinished { name, is_error } => {
                self.push_activity(format!(
                    "Finished tool: {name}{}",
                    progress_suffix(is_error)
                ));
                self.push_message(
                    Speaker::System,
                    format!("Finished tool: {name}{}", progress_suffix(is_error)),
                );
                self.status = Some(if is_error {
                    format!("Tool failed: {name}")
                } else {
                    format!("Tool finished: {name}")
                });
            }
            AgentProgress::CommandStarted { command, cwd } => {
                self.push_activity(format!("Running command in {cwd}: {command}"));
                self.push_message(
                    Speaker::System,
                    format!("Running command in {cwd}: {command}"),
                );
                self.status = Some(format!("Command: {}", truncate_text(&command, 36)));
            }
            AgentProgress::CommandFinished {
                command,
                exit_code,
                timed_out,
                stdout_preview,
                stderr_preview,
            } => {
                self.push_activity(format_command_progress_message(
                    &command, exit_code, timed_out,
                ));
                self.last_command_output = Some(CommandOutputState {
                    command: command.clone(),
                    exit_code,
                    timed_out,
                    stdout_preview,
                    stderr_preview,
                });
                self.push_message(
                    Speaker::System,
                    format_command_progress_message(&command, exit_code, timed_out),
                );
                self.status = Some(command_completion_status(&command, exit_code, timed_out));
            }
        }
    }

    async fn run(mut self, mut terminal: DefaultTerminal) -> Result<()> {
        loop {
            self.poll_pending_task();
            terminal.draw(|frame| self.draw(frame))?;

            if !event::poll(Duration::from_millis(50))? {
                continue;
            }

            if let Event::Key(key) = event::read()? {
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

    fn draw(&self, frame: &mut Frame) {
        frame.render_widget(Block::new().style(Style::default().bg(BG)), frame.area());

        let vertical = Layout::vertical([
            Constraint::Length(10),
            Constraint::Min(8),
            Constraint::Length(8),
            Constraint::Length(2),
            Constraint::Length(1),
        ]);
        let [
            header_area,
            transcript_area,
            lower_area,
            composer_area,
            footer_area,
        ] = vertical.areas(frame.area());
        let [activity_area, command_output_area] =
            Layout::horizontal([Constraint::Percentage(42), Constraint::Percentage(58)])
                .areas(lower_area);

        self.draw_header(frame, header_area);
        self.draw_transcript(frame, transcript_area);
        self.draw_activity(frame, activity_area);
        self.draw_command_output(frame, command_output_area);
        self.draw_composer(frame, composer_area);
        self.draw_footer(frame, footer_area);
    }

    fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let mut logo_lines = build_logo_rows(&LOGO_WORDMARK);
        logo_lines.push(Line::from(vec![
            Span::styled("model ", Style::default().fg(SOFT)),
            Span::styled(&self.model_label, Style::default().fg(BLUE)),
        ]));
        logo_lines.push(Line::from(vec![
            Span::styled("dir ", Style::default().fg(SOFT)),
            Span::styled(&self.workspace_label, Style::default().fg(LILAC)),
        ]));
        logo_lines.push(Line::from(vec![
            Span::styled("socium ", Style::default().fg(SOFT)),
            Span::styled(
                format!("v{}", env!("CARGO_PKG_VERSION")),
                Style::default().fg(PINK),
            ),
        ]));
        frame.render_widget(
            Paragraph::new(Text::from(logo_lines)).style(Style::default().bg(BG)),
            area,
        );
    }

    fn draw_transcript(&self, frame: &mut Frame, area: Rect) {
        let transcript =
            Paragraph::new(self.visible_transcript(area.width as usize, area.height as usize))
                .style(Style::default().bg(BG).fg(TEXT));
        frame.render_widget(transcript, area);
    }

    fn draw_activity(&self, frame: &mut Frame, area: Rect) {
        let lines =
            recent_activity_lines(&self.activity_log, area.height.saturating_sub(2) as usize);
        let block = Block::bordered()
            .title(" activity ")
            .border_style(Style::default().fg(CODE_BORDER))
            .style(Style::default().bg(BG));
        let text = if lines.is_empty() {
            Text::from(vec![Line::from(Span::styled(
                "No tool activity yet",
                Style::default().fg(SOFT),
            ))])
        } else {
            Text::from(
                lines
                    .into_iter()
                    .map(|line| {
                        Line::from(vec![
                            Span::styled("• ", Style::default().fg(PINK)),
                            Span::styled(line, Style::default().fg(TEXT)),
                        ])
                    })
                    .collect::<Vec<_>>(),
            )
        };
        frame.render_widget(Paragraph::new(text).block(block), area);
    }

    fn draw_command_output(&self, frame: &mut Frame, area: Rect) {
        let block = Block::bordered()
            .title(" last command ")
            .border_style(Style::default().fg(CODE_BORDER))
            .style(Style::default().bg(BG));
        let text = if let Some(output) = &self.last_command_output {
            Text::from(
                command_output_preview_lines(
                    output,
                    area.width.saturating_sub(2) as usize,
                    area.height.saturating_sub(2) as usize,
                )
                .into_iter()
                .map(|line| Line::from(Span::styled(line, Style::default().fg(TEXT))))
                .collect::<Vec<_>>(),
            )
        } else {
            Text::from(vec![Line::from(Span::styled(
                "No command output yet",
                Style::default().fg(SOFT),
            ))])
        };
        frame.render_widget(Paragraph::new(text).block(block), area);
    }

    fn draw_composer(&self, frame: &mut Frame, area: Rect) {
        let available_chars = area.width.saturating_sub(2) as usize;
        let (visible_input, visible_cursor) = self.visible_input_window(available_chars);

        let line = if self.input.is_empty() {
            Line::from(vec![
                Span::styled("> ", Style::default().fg(MUTED)),
                Span::styled(
                    "Ask Socium to inspect files, explain code, or run tasks",
                    Style::default().fg(SOFT),
                ),
            ])
        } else {
            Line::from(vec![
                Span::styled("> ", Style::default().fg(MUTED)),
                Span::styled(visible_input, Style::default().fg(MUTED)),
            ])
        };

        frame.render_widget(Paragraph::new(line).style(Style::default().bg(BG)), area);

        if matches!(self.input_mode, InputMode::Editing) {
            frame.set_cursor_position(Position::new(area.x + 2 + visible_cursor as u16, area.y));
        }
    }

    fn draw_footer(&self, frame: &mut Frame, area: Rect) {
        let [left, center, right] = Layout::horizontal([
            Constraint::Percentage(38),
            Constraint::Percentage(22),
            Constraint::Percentage(40),
        ])
        .areas(area);

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(&self.workspace_label, Style::default().fg(BLUE)),
                Span::styled(" ", Style::default().fg(MUTED)),
                Span::styled(
                    format!(
                        "({})",
                        self.status.clone().unwrap_or_else(|| "idle".to_string())
                    ),
                    Style::default().fg(MUTED),
                ),
            ]))
            .style(Style::default().bg(BG)),
            left,
        );

        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(
                "workspace-write (see /docs)",
                Style::default().fg(PINK),
            )]))
            .style(Style::default().bg(BG)),
            center,
        );

        let latency_label = self
            .last_latency_ms
            .map(|latency| format!("{latency}ms"))
            .unwrap_or_else(|| "--".to_string());
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(&self.model_label, Style::default().fg(BLUE)),
                Span::styled(" ", Style::default().fg(MUTED)),
                Span::styled(
                    format!(
                        "({}% context left, {latency_label})",
                        self.context_metrics.context_left_percentage
                    ),
                    Style::default().fg(MUTED),
                ),
                Span::styled(" ", Style::default().fg(MUTED)),
                Span::styled(
                    format!(
                        "{}/{}",
                        self.context_metrics.estimated_input_tokens,
                        self.context_metrics.input_budget_tokens
                    ),
                    Style::default().fg(SOFT),
                ),
                Span::styled(" ", Style::default().fg(MUTED)),
                Span::styled(
                    if self.context_metrics.summary_active {
                        format!("summary {}", self.context_metrics.summarized_messages)
                    } else {
                        "summary off".to_string()
                    },
                    Style::default().fg(PINK),
                ),
            ]))
            .style(Style::default().bg(BG)),
            right,
        );
    }

    fn visible_input_window(&self, width: usize) -> (String, usize) {
        if width == 0 {
            return (String::new(), 0);
        }

        let char_count = self.input.chars().count();
        if char_count <= width {
            return (self.input.clone(), self.character_index);
        }

        let start = self.character_index.saturating_sub(width);
        let visible = self
            .input
            .chars()
            .skip(start)
            .take(width)
            .collect::<String>();
        (visible, self.character_index.saturating_sub(start))
    }

    fn visible_transcript(&self, width: usize, height: usize) -> Text<'static> {
        if self.messages.is_empty() {
            return Text::from(vec![
                Line::from(Span::styled(
                    "Tips for getting started:",
                    Style::default().fg(TEXT),
                )),
                Line::from(Span::styled(
                    "1. Ask questions, edit files, or run commands.",
                    Style::default().fg(TEXT),
                )),
                Line::from(Span::styled(
                    "2. Be specific for the best results.",
                    Style::default().fg(TEXT),
                )),
                Line::from(vec![
                    Span::styled("3. ", Style::default().fg(TEXT)),
                    Span::styled("/help", Style::default().fg(LILAC)),
                    Span::styled(" for more information.", Style::default().fg(TEXT)),
                ]),
            ]);
        }

        let mut lines = Vec::new();
        for entry in &self.messages {
            lines.extend(render_entry(entry, width.max(24)));
        }

        let start = transcript_window_start(lines.len(), height.max(1), self.transcript_scroll);
        Text::from(lines.into_iter().skip(start).collect::<Vec<_>>())
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

    fn refresh_context_metrics(&mut self) {
        if let Some(agent) = self.agent.as_ref()
            && let Ok(metrics) = agent.context_metrics(&self.input)
        {
            self.context_metrics = metrics;
        }
    }
}

fn model_request_status(round: usize, provider: &str) -> String {
    if round <= 1 {
        format!("Thinking ({provider})")
    } else {
        format!("Thinking ({provider}, round {round})")
    }
}

fn append_streamed_assistant_text(messages: &mut Vec<ChatEntry>, text: &str) {
    if text.is_empty() {
        return;
    }

    if let Some(entry) = messages.last_mut()
        && entry.speaker == Speaker::Socium
    {
        entry.body.push_str(text);
        return;
    }

    messages.push(ChatEntry {
        speaker: Speaker::Socium,
        body: text.to_string(),
    });
}

fn progress_suffix(is_error: bool) -> &'static str {
    if is_error { " (error)" } else { "" }
}

fn format_command_progress_message(
    command: &str,
    exit_code: Option<i32>,
    timed_out: bool,
) -> String {
    if timed_out {
        return format!("Command timed out: {command}");
    }
    match exit_code {
        Some(code) => format!("Command finished ({code}): {command}"),
        None => format!("Command finished (terminated): {command}"),
    }
}

fn command_completion_status(command: &str, exit_code: Option<i32>, timed_out: bool) -> String {
    if timed_out {
        return format!("Command timed out: {}", truncate_text(command, 24));
    }
    match exit_code {
        Some(0) => format!("Command finished: {}", truncate_text(command, 24)),
        Some(code) => format!("Command failed ({code}): {}", truncate_text(command, 24)),
        None => format!("Command terminated: {}", truncate_text(command, 24)),
    }
}

fn transcript_entries_to_chat_entries(entries: Vec<TranscriptEntry>) -> Vec<ChatEntry> {
    entries
        .into_iter()
        .map(|entry| ChatEntry {
            speaker: match entry.role {
                TranscriptRole::User => Speaker::User,
                TranscriptRole::Assistant => Speaker::Socium,
                TranscriptRole::System => Speaker::System,
            },
            body: entry.text,
        })
        .collect()
}

fn transcript_window_start(total_lines: usize, height: usize, scroll: usize) -> usize {
    let visible_height = height.max(1);
    let max_start = total_lines.saturating_sub(visible_height);
    max_start.saturating_sub(scroll.min(max_start))
}

fn recent_activity_lines(entries: &[String], max_lines: usize) -> Vec<String> {
    if max_lines == 0 {
        return Vec::new();
    }
    let start = entries.len().saturating_sub(max_lines);
    entries[start..].to_vec()
}

fn command_output_preview_lines(
    output: &CommandOutputState,
    width: usize,
    max_lines: usize,
) -> Vec<String> {
    if max_lines == 0 {
        return Vec::new();
    }

    let wrap_width = width.max(12);
    let mut lines = vec![
        format!("$ {}", output.command),
        if output.timed_out {
            "status: timed out".to_string()
        } else {
            match output.exit_code {
                Some(code) => format!("status: exit {code}"),
                None => "status: terminated".to_string(),
            }
        },
    ];

    lines.push("stdout:".to_string());
    lines.extend(wrap_multiline(
        &output.stdout_preview,
        wrap_width,
        wrap_width,
    ));
    lines.push("stderr:".to_string());
    lines.extend(wrap_multiline(
        &output.stderr_preview,
        wrap_width,
        wrap_width,
    ));

    lines.into_iter().take(max_lines).collect()
}

fn help_text() -> &'static str {
    "/help  Show available commands\n/clear Clear the current conversation\n/exit  Quit Socium"
}

fn build_logo_rows(rows: &[&str]) -> Vec<Line<'static>> {
    rows.iter()
        .map(|row| build_logo_line(row))
        .collect::<Vec<_>>()
}

fn build_logo_line(row: &str) -> Line<'static> {
    let total = row.chars().count();
    let first_end = total * 33 / 100;
    let second_end = total * 72 / 100;
    let left = slice_chars(row, 0, first_end);
    let middle = slice_chars(row, first_end, second_end);
    let right = slice_chars(row, second_end, total);

    Line::from(vec![
        Span::styled(left, Style::default().fg(BLUE).add_modifier(Modifier::BOLD)),
        Span::styled(
            middle,
            Style::default().fg(LILAC).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            right,
            Style::default().fg(PINK).add_modifier(Modifier::BOLD),
        ),
    ])
}

fn slice_chars(value: &str, start: usize, end: usize) -> String {
    value
        .chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}

fn render_entry(entry: &ChatEntry, width: usize) -> Vec<Line<'static>> {
    match entry.speaker {
        Speaker::User => render_wrapped_text(&entry.body, width, "> ", "  ", MUTED, MUTED, None),
        Speaker::Socium => render_socium_entry(entry, width),
        Speaker::Thinking => render_thinking_card(&entry.body, width),
        Speaker::System => render_wrapped_text(&entry.body, width, "! ", "  ", PINK, TEXT, None),
    }
}

fn render_thinking_card(text: &str, width: usize) -> Vec<Line<'static>> {
    let card_width = width.saturating_sub(4).max(32);
    let inner_width = card_width.saturating_sub(4);
    let wrapped = wrap_multiline(text, inner_width, inner_width);
    let mut lines = vec![Line::from(vec![
        Span::styled("  ", Style::default().fg(MUTED)),
        Span::styled(
            format!("┌{}┐", "─".repeat(card_width.saturating_sub(2))),
            Style::default().fg(CODE_BORDER),
        ),
    ])];

    let title = "↪ SociumAgent";
    let subtitle = wrapped
        .first()
        .cloned()
        .unwrap_or_else(|| "Working on it".to_string());
    lines.push(card_line(&format!("{title} {subtitle}"), card_width));

    for line in wrapped.into_iter().skip(1) {
        lines.push(card_line(&line, card_width));
    }

    lines.push(Line::from(vec![
        Span::styled("  ", Style::default().fg(MUTED)),
        Span::styled(
            format!("└{}┘", "─".repeat(card_width.saturating_sub(2))),
            Style::default().fg(CODE_BORDER),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(": ", Style::default().fg(MUTED)),
        Span::styled("Socium is thinking", Style::default().fg(LILAC)),
        Span::styled(" (esc to cancel)", Style::default().fg(MUTED)),
    ]));
    lines.push(Line::from(""));
    lines
}

fn card_line(content: &str, card_width: usize) -> Line<'static> {
    let inner_width = card_width.saturating_sub(4);
    let visible = content.chars().take(inner_width).collect::<String>();
    let padding = inner_width.saturating_sub(visible.chars().count());

    Line::from(vec![
        Span::styled("  ", Style::default().fg(MUTED)),
        Span::styled("│ ", Style::default().fg(CODE_BORDER)),
        Span::styled(visible, Style::default().fg(CODE_TEXT)),
        Span::styled(
            format!("{} │", " ".repeat(padding)),
            Style::default().fg(CODE_BORDER),
        ),
    ])
}

fn render_socium_entry(entry: &ChatEntry, width: usize) -> Vec<Line<'static>> {
    let segments = parse_segments(&entry.body);
    let mut lines = Vec::new();
    let mut wrote_text = false;

    for segment in segments {
        match segment {
            Segment::Text(text) => {
                let wrapped =
                    wrap_multiline(&text, width.saturating_sub(2), width.saturating_sub(2));
                if wrapped.is_empty() {
                    continue;
                }

                if !wrote_text {
                    let first = wrapped.first().cloned().unwrap_or_default();
                    lines.push(Line::from(vec![
                        Span::styled("✦ ", Style::default().fg(LILAC)),
                        Span::styled(first, Style::default().fg(TEXT)),
                    ]));
                    for line in wrapped.into_iter().skip(1) {
                        lines.push(Line::from(vec![
                            Span::styled("  ", Style::default().fg(MUTED)),
                            Span::styled(line, Style::default().fg(TEXT)),
                        ]));
                    }
                    wrote_text = true;
                } else {
                    for line in wrapped {
                        lines.push(Line::from(vec![
                            Span::styled("  ", Style::default().fg(MUTED)),
                            Span::styled(line, Style::default().fg(TEXT)),
                        ]));
                    }
                }
            }
            Segment::Code { label, lines: code } => {
                lines.extend(render_code_block(&label, &code, width));
                wrote_text = true;
            }
        }
    }

    if !wrote_text {
        lines.push(Line::from(Span::styled("✦", Style::default().fg(LILAC))));
    }

    lines.push(Line::from(""));
    lines
}

fn render_wrapped_text(
    text: &str,
    width: usize,
    prefix: &str,
    indent: &str,
    prefix_color: Color,
    text_color: Color,
    timestamp: Option<&str>,
) -> Vec<Line<'static>> {
    let wrapped = wrap_multiline(
        text,
        width.saturating_sub(prefix.chars().count()),
        width.saturating_sub(indent.chars().count()),
    );
    let mut lines = Vec::new();

    if let Some(first) = wrapped.first() {
        let mut first_line = vec![
            Span::styled(prefix.to_string(), Style::default().fg(prefix_color)),
            Span::styled(first.clone(), Style::default().fg(text_color)),
        ];
        if let Some(ts) = timestamp {
            first_line.push(Span::styled(format!("  {ts}"), Style::default().fg(SOFT)));
        }
        lines.push(Line::from(first_line));
    }

    for line in wrapped.into_iter().skip(1) {
        lines.push(Line::from(vec![
            Span::styled(indent.to_string(), Style::default().fg(MUTED)),
            Span::styled(line, Style::default().fg(text_color)),
        ]));
    }

    lines.push(Line::from(""));
    lines
}

fn parse_segments(body: &str) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut text_buffer = Vec::new();
    let mut code_buffer = Vec::new();
    let mut code_label = String::new();
    let mut in_code = false;

    for line in body.lines() {
        if let Some(info) = line.strip_prefix("```") {
            if in_code {
                segments.push(Segment::Code {
                    label: if code_label.is_empty() {
                        "snippet".to_string()
                    } else {
                        code_label.clone()
                    },
                    lines: code_buffer.clone(),
                });
                code_buffer.clear();
                code_label.clear();
                in_code = false;
            } else {
                if !text_buffer.is_empty() {
                    segments.push(Segment::Text(text_buffer.join("\n")));
                    text_buffer.clear();
                }
                code_label = info.trim().to_string();
                in_code = true;
            }
            continue;
        }

        if in_code {
            code_buffer.push(line.to_string());
        } else {
            text_buffer.push(line.to_string());
        }
    }

    if !text_buffer.is_empty() {
        segments.push(Segment::Text(text_buffer.join("\n")));
    }

    if in_code {
        segments.push(Segment::Code {
            label: if code_label.is_empty() {
                "snippet".to_string()
            } else {
                code_label
            },
            lines: code_buffer,
        });
    }

    if segments.is_empty() {
        segments.push(Segment::Text(body.to_string()));
    }

    segments
}

fn wrap_multiline(text: &str, first_width: usize, next_width: usize) -> Vec<String> {
    let mut result = Vec::new();
    let mut current_width = first_width.max(1);

    for paragraph in text.lines() {
        if paragraph.is_empty() {
            result.push(String::new());
            current_width = next_width.max(1);
            continue;
        }

        let wrapped = wrap_paragraph(paragraph, current_width, next_width.max(1));
        result.extend(wrapped);
        current_width = next_width.max(1);
    }

    result
}

fn wrap_paragraph(paragraph: &str, first_width: usize, next_width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut width = first_width.max(1);

    for word in paragraph.split_whitespace() {
        for segment in split_long_word(word, width.max(1)) {
            if current.is_empty() {
                current.push_str(&segment);
                continue;
            }

            let next_len = current.chars().count() + 1 + segment.chars().count();
            if next_len <= width {
                current.push(' ');
                current.push_str(&segment);
            } else {
                lines.push(std::mem::take(&mut current));
                current.push_str(&segment);
                width = next_width.max(1);
            }
        }
    }

    if !current.is_empty() {
        lines.push(current);
    }

    lines
}

fn split_long_word(word: &str, width: usize) -> Vec<String> {
    if word.chars().count() <= width {
        return vec![word.to_string()];
    }

    let mut parts = Vec::new();
    let mut current = String::new();
    for ch in word.chars() {
        current.push(ch);
        if current.chars().count() == width {
            parts.push(std::mem::take(&mut current));
        }
    }

    if !current.is_empty() {
        parts.push(current);
    }

    parts
}

fn render_code_block(label: &str, code_lines: &[String], width: usize) -> Vec<Line<'static>> {
    let outer_width = width.saturating_sub(2).max(24);
    let inner_width = outer_width.saturating_sub(2);
    let title = format!(" {} ", if label.is_empty() { "snippet" } else { label });
    let title_chars = title.chars().count().min(inner_width);
    let title_text = title.chars().take(title_chars).collect::<String>();
    let fill = inner_width.saturating_sub(title_text.chars().count());

    let mut lines = vec![Line::from(vec![
        Span::styled("  ", Style::default().fg(MUTED)),
        Span::styled(
            format!("┌{}{}┐", title_text, "─".repeat(fill)),
            Style::default().fg(CODE_BORDER),
        ),
    ])];

    for raw_line in code_lines {
        let visible = raw_line
            .chars()
            .take(inner_width.saturating_sub(2))
            .collect::<String>();
        let padding = inner_width.saturating_sub(2 + visible.chars().count());
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default().fg(MUTED)),
            Span::styled("│ ", Style::default().fg(CODE_BORDER)),
            Span::styled(visible, Style::default().fg(CODE_TEXT)),
            Span::styled(
                format!("{} │", " ".repeat(padding)),
                Style::default().fg(CODE_BORDER),
            ),
        ]));
    }

    lines.push(Line::from(vec![
        Span::styled("  ", Style::default().fg(MUTED)),
        Span::styled(
            format!("└{}┘", "─".repeat(inner_width)),
            Style::default().fg(CODE_BORDER),
        ),
    ]));
    lines.push(Line::from(""));

    lines
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

fn truncate_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }

    let mut truncated = text
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use super::{
        ChatEntry, CommandOutputState, Speaker, append_streamed_assistant_text,
        command_output_preview_lines, recent_activity_lines, transcript_entries_to_chat_entries,
        transcript_window_start,
    };
    use super::{command_completion_status, format_command_progress_message, model_request_status};
    use socium_core::{TranscriptEntry, TranscriptRole};

    #[test]
    fn model_request_status_includes_round_after_first_request() {
        assert_eq!(model_request_status(1, "anthropic"), "Thinking (anthropic)");
        assert_eq!(
            model_request_status(2, "anthropic"),
            "Thinking (anthropic, round 2)"
        );
    }

    #[test]
    fn command_completion_status_reports_failures_and_timeouts() {
        assert_eq!(
            command_completion_status("cargo test", Some(0), false),
            "Command finished: cargo test"
        );
        assert_eq!(
            command_completion_status("cargo test", Some(101), false),
            "Command failed (101): cargo test"
        );
        assert_eq!(
            command_completion_status("cargo test", None, true),
            "Command timed out: cargo test"
        );
    }

    #[test]
    fn format_command_progress_message_reports_exit_code() {
        assert_eq!(
            format_command_progress_message("cargo test", Some(0), false),
            "Command finished (0): cargo test"
        );
        assert_eq!(
            format_command_progress_message("cargo test", None, true),
            "Command timed out: cargo test"
        );
    }

    #[test]
    fn help_text_lists_supported_slash_commands() {
        let help = super::help_text();

        assert!(help.contains("/help"));
        assert!(help.contains("/clear"));
        assert!(help.contains("/exit"));
    }

    #[test]
    fn transcript_entries_map_to_chat_speakers() {
        let entries = vec![
            TranscriptEntry {
                role: TranscriptRole::User,
                text: "u".to_string(),
            },
            TranscriptEntry {
                role: TranscriptRole::Assistant,
                text: "a".to_string(),
            },
            TranscriptEntry {
                role: TranscriptRole::System,
                text: "s".to_string(),
            },
        ];

        let chat = transcript_entries_to_chat_entries(entries);

        assert_eq!(chat.len(), 3);
        assert!(matches!(chat[0].speaker, Speaker::User));
        assert!(matches!(chat[1].speaker, Speaker::Socium));
        assert!(matches!(chat[2].speaker, Speaker::System));
    }

    #[test]
    fn transcript_window_start_respects_scroll_offset() {
        assert_eq!(transcript_window_start(20, 5, 0), 15);
        assert_eq!(transcript_window_start(20, 5, 3), 12);
        assert_eq!(transcript_window_start(20, 5, 99), 0);
        assert_eq!(transcript_window_start(3, 10, 5), 0);
    }

    #[test]
    fn recent_activity_lines_keeps_latest_entries() {
        let entries = vec![
            "one".to_string(),
            "two".to_string(),
            "three".to_string(),
            "four".to_string(),
        ];

        assert_eq!(
            recent_activity_lines(&entries, 2),
            vec!["three".to_string(), "four".to_string()]
        );
        assert_eq!(recent_activity_lines(&entries, 0), Vec::<String>::new());
    }

    #[test]
    fn command_output_preview_lines_include_status_and_streams() {
        let output = CommandOutputState {
            command: "cargo test".to_string(),
            exit_code: Some(0),
            timed_out: false,
            stdout_preview: "ok".to_string(),
            stderr_preview: "(empty)".to_string(),
        };

        let lines = command_output_preview_lines(&output, 40, 6);

        assert_eq!(lines[0], "$ cargo test");
        assert_eq!(lines[1], "status: exit 0");
        assert!(lines.contains(&"stdout:".to_string()));
        assert!(lines.contains(&"stderr:".to_string()));
    }

    #[test]
    fn append_streamed_assistant_text_appends_to_latest_assistant_entry() {
        let mut messages = vec![
            ChatEntry {
                speaker: Speaker::User,
                body: "user".to_string(),
            },
            ChatEntry {
                speaker: Speaker::Socium,
                body: "hel".to_string(),
            },
        ];

        append_streamed_assistant_text(&mut messages, "lo");

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].body, "hello");
    }

    #[test]
    fn append_streamed_assistant_text_pushes_new_entry_after_system_message() {
        let mut messages = vec![ChatEntry {
            speaker: Speaker::System,
            body: "tool finished".to_string(),
        }];

        append_streamed_assistant_text(&mut messages, "answer");

        assert_eq!(messages.len(), 2);
        assert!(matches!(messages[1].speaker, Speaker::Socium));
        assert_eq!(messages[1].body, "answer");
    }
}
