//! Activity pane state: core activity events folded into an ordered list of
//! items, with selection and expansion for browsing.

use crate::transcript::CommandOutputState;
use socium_core::activity::{Phase, SignalChange, SignalTracker};
use socium_core::providers::TokenUsage;
use socium_core::{ActivityEvent, ActivityKind, ThoughtRole};
use std::collections::VecDeque;

/// Text kept per item; the full reasoning stays in the conversation history.
const MAX_ITEM_TEXT_BYTES: usize = 64 * 1024;
const MAX_ITEMS: usize = 2_000;
/// Raw provider events are kept for the most recent rounds only.
const RAW_ROUNDS: usize = 10;
const MAX_RAW_BYTES_PER_ROUND: usize = 2 * 1024 * 1024;

/// Provider stream events of one model request, as received.
pub(crate) struct RawRound {
    pub(crate) turn_id: usize,
    pub(crate) round: usize,
    pub(crate) events: Vec<String>,
    bytes: usize,
    pub(crate) truncated: bool,
}

/// The raw stream of one round shown in place of the item list.
pub(crate) struct RawView {
    pub(crate) turn_id: usize,
    pub(crate) round: usize,
    /// Lines scrolled up from the end.
    pub(crate) scroll: usize,
}

pub(crate) struct Item {
    pub(crate) turn_id: usize,
    pub(crate) round: usize,
    /// Call ID of the `agent` call whose subagent this item belongs to.
    pub(crate) agent: Option<String>,
    pub(crate) kind: ItemKind,
    pub(crate) expanded: bool,
}

pub(crate) enum ItemKind {
    Turn {
        number: usize,
        replayed: bool,
        outcome: Option<TurnOutcome>,
    },
    Round {
        model: String,
        stats: Option<RoundStats>,
    },
    Thought {
        block: usize,
        role: Option<ThoughtRole>,
        text: String,
        truncated: bool,
        interrupted: bool,
    },
    Response {
        chars: usize,
    },
    Tool(Box<ToolItem>),
    Note(String),
    /// A pattern derived from the items before it (ADR 5).
    Signal(String),
}

pub(crate) struct TurnOutcome {
    pub(crate) ok: bool,
    pub(crate) error: Option<String>,
    pub(crate) elapsed_ms: u64,
}

pub(crate) struct RoundStats {
    pub(crate) stop_reason: Option<String>,
    pub(crate) usage: Option<TokenUsage>,
    pub(crate) ttft_ms: Option<u64>,
    pub(crate) elapsed_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ToolState {
    /// The model is still writing the input.
    Composing,
    /// Input complete; waiting for the round to end before running.
    Ready,
    Running,
    Finished {
        is_error: bool,
        elapsed_ms: u64,
        summary: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ApprovalState {
    Waiting,
    Resolved { approved: bool, waited_ms: u64 },
}

pub(crate) struct ToolItem {
    pub(crate) call_id: String,
    pub(crate) block: Option<usize>,
    pub(crate) name: String,
    pub(crate) partial_json: String,
    pub(crate) input_summary: String,
    pub(crate) state: ToolState,
    pub(crate) approval: Option<ApprovalState>,
    pub(crate) output_bytes: Option<(u64, u64)>,
    pub(crate) command_output: Option<CommandOutputState>,
}

#[derive(Default)]
pub(crate) struct ActivityModel {
    pub(crate) items: Vec<Item>,
    /// `None` follows the newest item.
    pub(crate) selected: Option<usize>,
    pub(crate) raw: VecDeque<RawRound>,
    pub(crate) raw_view: Option<RawView>,
    signals: SignalTracker,
    turns_seen: usize,
}

impl ActivityModel {
    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub(crate) fn phase(&self) -> Phase {
        self.signals.phase()
    }

    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    /// Removes turn `turn_id` and the later ones, which a rewind dropped.
    pub(crate) fn drop_turns_from(&mut self, turn_id: usize) {
        self.items.retain(|item| item.turn_id < turn_id);
        self.raw.retain(|raw| raw.turn_id < turn_id);
        if self
            .raw_view
            .as_ref()
            .is_some_and(|view| view.turn_id >= turn_id)
        {
            self.raw_view = None;
        }
        self.selected = self.selected.filter(|&index| index < self.items.len());
        self.turns_seen = self
            .items
            .iter()
            .filter(|item| matches!(item.kind, ItemKind::Turn { .. }))
            .count();
    }

    /// A free-standing line, such as a request failure the core reported
    /// outside a turn.
    pub(crate) fn push_note(&mut self, text: impl Into<String>) {
        let (turn_id, round) = self.items.last().map_or((0, 0), |item| (item.turn_id, 0));
        self.push(turn_id, round, None, ItemKind::Note(text.into()));
    }

    pub(crate) fn apply(&mut self, event: ActivityEvent) {
        self.apply_with(event, false);
    }

    /// Replays a saved event; turns it opens are marked as past turns.
    pub(crate) fn apply_replayed(&mut self, event: ActivityEvent) {
        self.apply_with(event, true);
    }

    pub(crate) fn push_raw(&mut self, turn_id: usize, round: usize, json: String) {
        if self
            .raw
            .back()
            .is_none_or(|r| r.turn_id != turn_id || r.round != round)
        {
            if self.raw.len() == RAW_ROUNDS {
                self.raw.pop_front();
            }
            self.raw.push_back(RawRound {
                turn_id,
                round,
                events: Vec::new(),
                bytes: 0,
                truncated: false,
            });
        }
        let Some(current) = self.raw.back_mut() else {
            return;
        };
        if current.bytes + json.len() > MAX_RAW_BYTES_PER_ROUND {
            current.truncated = true;
            return;
        }
        current.bytes += json.len();
        current.events.push(json);
    }

    pub(crate) fn raw_round(&self, turn_id: usize, round: usize) -> Option<&RawRound> {
        self.raw
            .iter()
            .find(|r| r.turn_id == turn_id && r.round == round)
    }

    /// Opens the raw stream of the selected item's round. Returns why it
    /// cannot when the item has no model request or its stream is gone.
    pub(crate) fn open_raw_view(&mut self) -> Result<(), &'static str> {
        let item = self
            .selected_index()
            .and_then(|index| self.items.get(index))
            .ok_or("Select an item first")?;
        if item.round == 0 {
            return Err("This item is not part of a model request");
        }
        if item.agent.is_some() {
            return Err("Raw streams of subagent rounds are not kept");
        }
        if self.raw_round(item.turn_id, item.round).is_none() {
            return Err("The raw stream of this round is no longer kept");
        }
        self.raw_view = Some(RawView {
            turn_id: item.turn_id,
            round: item.round,
            scroll: 0,
        });
        Ok(())
    }

    fn apply_with(&mut self, event: ActivityEvent, replayed: bool) {
        let raised = self
            .signals
            .observe(&event)
            .into_iter()
            .filter_map(|change| match change {
                SignalChange::Raised(signal) => Some(signal.describe()),
                SignalChange::Phase(_) => None,
            })
            .collect::<Vec<_>>();
        let (signal_turn, signal_round, signal_agent) =
            (event.turn_id, event.round, event.agent.clone());
        self.apply_kind(event, replayed);
        for description in raised {
            self.push(
                signal_turn,
                signal_round,
                signal_agent.clone(),
                ItemKind::Signal(description),
            );
        }
    }

    fn apply_kind(&mut self, event: ActivityEvent, replayed: bool) {
        let ActivityEvent {
            turn_id,
            round,
            at_ms,
            agent,
            kind,
        } = event;
        match kind {
            ActivityKind::TurnStarted => {
                self.turns_seen += 1;
                let number = self.turns_seen;
                self.push(
                    turn_id,
                    round,
                    agent.clone(),
                    ItemKind::Turn {
                        number,
                        replayed,
                        outcome: None,
                    },
                );
            }
            ActivityKind::RoundStarted { model } => {
                self.push(
                    turn_id,
                    round,
                    agent,
                    ItemKind::Round { model, stats: None },
                );
            }
            ActivityKind::ThoughtStarted { block } => self.push(
                turn_id,
                round,
                agent.clone(),
                ItemKind::Thought {
                    block,
                    role: None,
                    text: String::new(),
                    truncated: false,
                    interrupted: false,
                },
            ),
            ActivityKind::ThoughtDelta { block, text: delta } => {
                if let Some(ItemKind::Thought {
                    text, truncated, ..
                }) = self.find_thought(turn_id, round, agent.as_deref(), block)
                {
                    append_bounded(text, truncated, &delta);
                }
            }
            ActivityKind::ThoughtFinished {
                block,
                role: finished_role,
                interrupted: was_interrupted,
            } => {
                if let Some(ItemKind::Thought {
                    role, interrupted, ..
                }) = self.find_thought(turn_id, round, agent.as_deref(), block)
                {
                    *role = Some(finished_role);
                    *interrupted = was_interrupted;
                }
            }
            ActivityKind::TextFinished { chars, .. } => {
                if chars > 0 {
                    self.push(turn_id, round, agent, ItemKind::Response { chars });
                }
            }
            ActivityKind::ToolIntentStarted {
                block,
                call_id,
                name,
            } => self.push(
                turn_id,
                round,
                agent.clone(),
                ItemKind::Tool(Box::new(ToolItem {
                    call_id,
                    block: Some(block),
                    name,
                    partial_json: String::new(),
                    input_summary: String::new(),
                    state: ToolState::Composing,
                    approval: None,
                    output_bytes: None,
                    command_output: None,
                })),
            ),
            ActivityKind::ToolIntentDelta {
                block,
                partial_json,
            } => {
                if let Some(tool) = self.find_tool_by_block(turn_id, round, agent.as_deref(), block)
                {
                    let mut truncated = false;
                    append_bounded(&mut tool.partial_json, &mut truncated, &partial_json);
                    if let Some(summary) = socium_core::activity::summarize_partial_tool_input(
                        &tool.name,
                        &tool.partial_json,
                    ) {
                        tool.input_summary = summary;
                    }
                }
            }
            ActivityKind::ToolIntentFinished { block } => {
                if let Some(tool) = self.find_tool_by_block(turn_id, round, agent.as_deref(), block)
                    && tool.state == ToolState::Composing
                {
                    tool.state = ToolState::Ready;
                }
            }
            ActivityKind::RoundFinished {
                stop_reason,
                usage,
                ttft_ms,
                elapsed_ms,
            } => {
                if let Some(Item {
                    kind: ItemKind::Round { stats, .. },
                    ..
                }) = self.items.iter_mut().rev().find(|item| {
                    item.turn_id == turn_id
                        && item.round == round
                        && item.agent == agent
                        && matches!(item.kind, ItemKind::Round { .. })
                }) {
                    *stats = Some(RoundStats {
                        stop_reason,
                        usage,
                        ttft_ms,
                        elapsed_ms,
                    });
                }
            }
            ActivityKind::ToolStarted {
                call_id,
                name,
                input_summary,
            } => {
                if let Some(tool) = self.find_tool(turn_id, agent.as_deref(), &call_id) {
                    tool.state = ToolState::Running;
                    if !input_summary.is_empty() {
                        tool.input_summary = input_summary;
                    }
                } else {
                    self.push(
                        turn_id,
                        round,
                        agent.clone(),
                        ItemKind::Tool(Box::new(ToolItem {
                            call_id,
                            block: None,
                            name,
                            partial_json: String::new(),
                            input_summary,
                            state: ToolState::Running,
                            approval: None,
                            output_bytes: None,
                            command_output: None,
                        })),
                    );
                }
            }
            ActivityKind::ApprovalWaiting { call_id, .. } => {
                if let Some(tool) = self.find_tool(turn_id, agent.as_deref(), &call_id) {
                    tool.approval = Some(ApprovalState::Waiting);
                }
            }
            ActivityKind::ApprovalResolved {
                call_id,
                approved,
                waited_ms,
            } => {
                if let Some(tool) = self.find_tool(turn_id, agent.as_deref(), &call_id) {
                    tool.approval = Some(ApprovalState::Resolved {
                        approved,
                        waited_ms,
                    });
                }
            }
            ActivityKind::CommandOutput {
                call_id,
                stdout_bytes,
                stderr_bytes,
            } => {
                if let Some(tool) = self.find_tool(turn_id, agent.as_deref(), &call_id) {
                    tool.output_bytes = Some((stdout_bytes, stderr_bytes));
                }
            }
            ActivityKind::ToolFinished {
                call_id,
                is_error,
                elapsed_ms,
                summary,
                ..
            } => {
                if let Some(tool) = self.find_tool(turn_id, agent.as_deref(), &call_id) {
                    tool.state = ToolState::Finished {
                        is_error,
                        elapsed_ms,
                        summary,
                    };
                }
            }
            ActivityKind::CompactionStarted => self.push(
                turn_id,
                round,
                agent.clone(),
                ItemKind::Note("Compacting conversation context".into()),
            ),
            ActivityKind::CompactionFinished {
                covered_messages,
                used_llm,
            } => self.push(
                turn_id,
                round,
                agent.clone(),
                ItemKind::Note(format!(
                    "Compacted: {covered_messages} messages summarized ({})",
                    if used_llm {
                        "model summary"
                    } else {
                        "deterministic fallback"
                    }
                )),
            ),
            ActivityKind::Warning { message } => {
                self.push(turn_id, round, agent, ItemKind::Note(message));
            }
            // The Timeline shows the plan; the `update_plan` call is a
            // tool item already.
            ActivityKind::PlanUpdated { .. } => {}
            ActivityKind::PlanNudged { rounds } => self.push(
                turn_id,
                round,
                agent,
                ItemKind::Signal(format!(
                    "Plan not updated for {rounds} rounds; the agent was reminded"
                )),
            ),
            ActivityKind::TurnFinished { ok, error } => {
                if let Some(Item {
                    kind: ItemKind::Turn { outcome, .. },
                    ..
                }) = self.items.iter_mut().rev().find(|item| {
                    item.turn_id == turn_id && matches!(item.kind, ItemKind::Turn { .. })
                }) {
                    *outcome = Some(TurnOutcome {
                        ok,
                        error,
                        elapsed_ms: at_ms,
                    });
                }
            }
        }
    }

    /// Attaches a finished command's output preview to the running
    /// `run_command` call, shown when that item is expanded.
    pub(crate) fn attach_command_output(&mut self, output: CommandOutputState) {
        if let Some(tool) = self
            .items
            .iter_mut()
            .rev()
            .find_map(|item| match &mut item.kind {
                ItemKind::Tool(tool)
                    if tool.name == "run_command" && tool.state == ToolState::Running =>
                {
                    Some(tool)
                }
                _ => None,
            })
        {
            tool.command_output = Some(output);
        }
    }

    pub(crate) fn select_by(&mut self, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        let last = self.items.len() - 1;
        let current = self.selected.unwrap_or(last);
        let next = current.saturating_add_signed(delta).min(last);
        // Selecting the newest item resumes following.
        self.selected = (next != last).then_some(next);
    }

    pub(crate) fn select_first(&mut self) {
        if self.items.len() > 1 {
            self.selected = Some(0);
        }
    }

    pub(crate) fn select_last(&mut self) {
        self.selected = None;
    }

    /// The selected item, or the newest one when following. The view marks
    /// it only while the pane is focused.
    pub(crate) fn selected_index(&self) -> Option<usize> {
        self.selected.or_else(|| self.items.len().checked_sub(1))
    }

    pub(crate) fn toggle_expanded(&mut self) {
        if let Some(index) = self.selected_index()
            && let Some(item) = self.items.get_mut(index)
        {
            item.expanded = !item.expanded;
        }
    }

    fn push(&mut self, turn_id: usize, round: usize, agent: Option<String>, kind: ItemKind) {
        let index = self.insertion_index(turn_id, agent.as_deref());
        if let Some(selected) = &mut self.selected
            && *selected >= index
        {
            *selected += 1;
        }
        self.items.insert(
            index,
            Item {
                turn_id,
                round,
                agent,
                kind,
                expanded: false,
            },
        );
        if self.items.len() > MAX_ITEMS {
            let excess = self.items.len() - MAX_ITEMS;
            self.items.drain(..excess);
            self.selected = self.selected.and_then(|s| s.checked_sub(excess));
        }
    }

    /// A subagent's items follow its `agent` call and its earlier items, so
    /// subagents running at the same time each read as one block.
    fn insertion_index(&self, turn_id: usize, agent: Option<&str>) -> usize {
        let parent = agent.and_then(|agent| {
            self.items.iter().rposition(|item| {
                item.turn_id == turn_id
                    && item.agent.is_none()
                    && matches!(&item.kind, ItemKind::Tool(tool) if tool.call_id == agent)
            })
        });
        let Some(parent) = parent else {
            return self.items.len();
        };
        let children = self.items[parent + 1..]
            .iter()
            .take_while(|item| item.agent.as_deref() == agent)
            .count();
        parent + 1 + children
    }

    fn find_thought(
        &mut self,
        turn_id: usize,
        round: usize,
        agent: Option<&str>,
        block: usize,
    ) -> Option<&mut ItemKind> {
        self.items
            .iter_mut()
            .rev()
            .take_while(|item| item.turn_id == turn_id)
            .find(|item| {
                item.round == round
                    && item.agent.as_deref() == agent
                    && matches!(item.kind, ItemKind::Thought { block: b, .. } if b == block)
            })
            .map(|item| &mut item.kind)
    }

    fn find_tool_by_block(
        &mut self,
        turn_id: usize,
        round: usize,
        agent: Option<&str>,
        block: usize,
    ) -> Option<&mut ToolItem> {
        self.items
            .iter_mut()
            .rev()
            .take_while(|item| item.turn_id == turn_id)
            .filter(|item| item.round == round && item.agent.as_deref() == agent)
            .find_map(|item| match &mut item.kind {
                ItemKind::Tool(tool) if tool.block == Some(block) => Some(tool.as_mut()),
                _ => None,
            })
    }

    fn find_tool(
        &mut self,
        turn_id: usize,
        agent: Option<&str>,
        call_id: &str,
    ) -> Option<&mut ToolItem> {
        self.items
            .iter_mut()
            .rev()
            .take_while(|item| item.turn_id == turn_id)
            .filter(|item| item.agent.as_deref() == agent)
            .find_map(|item| match &mut item.kind {
                ItemKind::Tool(tool) if tool.call_id == call_id => Some(tool.as_mut()),
                _ => None,
            })
    }
}

fn append_bounded(text: &mut String, truncated: &mut bool, delta: &str) {
    if *truncated {
        return;
    }
    let room = MAX_ITEM_TEXT_BYTES.saturating_sub(text.len());
    if delta.len() <= room {
        text.push_str(delta);
        return;
    }
    let mut end = room;
    while !delta.is_char_boundary(end) {
        end -= 1;
    }
    text.push_str(&delta[..end]);
    *truncated = true;
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn event(turn_id: usize, round: usize, kind: ActivityKind) -> ActivityEvent {
        ActivityEvent {
            turn_id,
            round,
            at_ms: 1_500,
            agent: None,
            kind,
        }
    }

    /// One turn: a progress update, a streamed read_file call that fails,
    /// then a final round with reasoning and a response.
    pub(crate) fn sample_turn() -> Vec<ActivityEvent> {
        vec![
            event(4, 0, ActivityKind::TurnStarted),
            event(
                4,
                1,
                ActivityKind::RoundStarted {
                    model: "claude-opus-5-5".into(),
                },
            ),
            event(4, 1, ActivityKind::ThoughtStarted { block: 0 }),
            event(
                4,
                1,
                ActivityKind::ThoughtDelta {
                    block: 0,
                    text: "Check how the view draws activity.".into(),
                },
            ),
            event(
                4,
                1,
                ActivityKind::ThoughtFinished {
                    block: 0,
                    role: ThoughtRole::ProgressUpdate,
                    interrupted: false,
                },
            ),
            event(
                4,
                1,
                ActivityKind::ToolIntentStarted {
                    block: 1,
                    call_id: "t1".into(),
                    name: "read_file".into(),
                },
            ),
            event(
                4,
                1,
                ActivityKind::ToolIntentDelta {
                    block: 1,
                    partial_json: "{\"path\":\"crates/tui/src/vi".into(),
                },
            ),
            event(
                4,
                1,
                ActivityKind::ToolIntentDelta {
                    block: 1,
                    partial_json: "ew/mod.rs\"}".into(),
                },
            ),
            event(4, 1, ActivityKind::ToolIntentFinished { block: 1 }),
            event(
                4,
                1,
                ActivityKind::RoundFinished {
                    stop_reason: Some("tool_use".into()),
                    usage: Some(TokenUsage {
                        input_tokens: 2_000,
                        output_tokens: 412,
                        cache_creation_input_tokens: 0,
                        cache_read_input_tokens: 16_000,
                    }),
                    ttft_ms: Some(1_200),
                    elapsed_ms: 6_800,
                },
            ),
            event(
                4,
                1,
                ActivityKind::ToolStarted {
                    call_id: "t1".into(),
                    name: "read_file".into(),
                    input_summary: "crates/tui/src/view/mod.rs".into(),
                },
            ),
            event(
                4,
                1,
                ActivityKind::ToolFinished {
                    call_id: "t1".into(),
                    name: "read_file".into(),
                    is_error: true,
                    elapsed_ms: 12,
                    summary: "File not found".into(),
                },
            ),
            event(
                4,
                2,
                ActivityKind::RoundStarted {
                    model: "claude-opus-5-5".into(),
                },
            ),
            event(4, 2, ActivityKind::ThoughtStarted { block: 0 }),
            event(
                4,
                2,
                ActivityKind::ThoughtDelta {
                    block: 0,
                    text: "The path was wrong; answer from what I know.".into(),
                },
            ),
            event(
                4,
                2,
                ActivityKind::ThoughtFinished {
                    block: 0,
                    role: ThoughtRole::Reasoning,
                    interrupted: false,
                },
            ),
            event(
                4,
                2,
                ActivityKind::TextFinished {
                    block: 1,
                    chars: 312,
                },
            ),
            event(
                4,
                2,
                ActivityKind::RoundFinished {
                    stop_reason: Some("end_turn".into()),
                    usage: None,
                    ttft_ms: Some(900),
                    elapsed_ms: 2_100,
                },
            ),
            event(
                4,
                0,
                ActivityKind::TurnFinished {
                    ok: true,
                    error: None,
                },
            ),
        ]
    }

    pub(crate) fn model_with(events: Vec<ActivityEvent>) -> ActivityModel {
        let mut model = ActivityModel::default();
        for event in events {
            model.apply(event);
        }
        model
    }

    #[test]
    fn a_streamed_tool_call_is_one_item_from_intent_to_result() {
        let model = model_with(sample_turn());

        let tools = model
            .items
            .iter()
            .filter_map(|item| match &item.kind {
                ItemKind::Tool(tool) => Some(tool),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(tools.len(), 1);
        assert_eq!(
            tools[0].partial_json,
            "{\"path\":\"crates/tui/src/view/mod.rs\"}"
        );
        assert_eq!(tools[0].input_summary, "crates/tui/src/view/mod.rs");
        assert_eq!(
            tools[0].state,
            ToolState::Finished {
                is_error: true,
                elapsed_ms: 12,
                summary: "File not found".into()
            }
        );
    }

    #[test]
    fn rounds_and_turns_receive_their_closing_stats() {
        let model = model_with(sample_turn());

        let kinds = model
            .items
            .iter()
            .map(|item| match &item.kind {
                ItemKind::Turn { outcome, .. } => {
                    format!("turn ok={:?}", outcome.as_ref().map(|o| o.ok))
                }
                ItemKind::Round { stats, .. } => format!(
                    "round {} {:?}",
                    item.round,
                    stats.as_ref().and_then(|s| s.stop_reason.clone())
                ),
                ItemKind::Thought { role, .. } => format!("thought {role:?}"),
                ItemKind::Response { chars } => format!("response {chars}"),
                ItemKind::Tool(tool) => format!("tool {}", tool.name),
                ItemKind::Note(text) => format!("note {text}"),
                ItemKind::Signal(text) => format!("signal {text}"),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            [
                "turn ok=Some(true)",
                "round 1 Some(\"tool_use\")",
                "thought Some(ProgressUpdate)",
                "tool read_file",
                "round 2 Some(\"end_turn\")",
                "thought Some(Reasoning)",
                "response 312",
            ]
        );
    }

    #[test]
    fn a_tool_without_streamed_intent_is_created_when_it_starts() {
        let mut model = ActivityModel::default();
        model.apply(event(
            1,
            1,
            ActivityKind::ToolStarted {
                call_id: "c".into(),
                name: "run_command".into(),
                input_summary: "cargo test".into(),
            },
        ));
        model.apply(event(
            1,
            1,
            ActivityKind::ApprovalWaiting {
                call_id: "c".into(),
                tool: "run_command".into(),
            },
        ));
        model.apply(event(
            1,
            1,
            ActivityKind::ApprovalResolved {
                call_id: "c".into(),
                approved: false,
                waited_ms: 3_000,
            },
        ));

        let ItemKind::Tool(tool) = &model.items[0].kind else {
            panic!("expected a tool item");
        };
        assert_eq!(tool.state, ToolState::Running);
        assert_eq!(
            tool.approval,
            Some(ApprovalState::Resolved {
                approved: false,
                waited_ms: 3_000
            })
        );
    }

    #[test]
    fn thought_text_is_bounded_on_a_char_boundary() {
        let mut model = ActivityModel::default();
        model.apply(event(1, 1, ActivityKind::ThoughtStarted { block: 0 }));
        model.apply(event(
            1,
            1,
            ActivityKind::ThoughtDelta {
                block: 0,
                text: "あ".repeat(MAX_ITEM_TEXT_BYTES),
            },
        ));
        model.apply(event(
            1,
            1,
            ActivityKind::ThoughtDelta {
                block: 0,
                text: "more".into(),
            },
        ));

        let ItemKind::Thought {
            text, truncated, ..
        } = &model.items[0].kind
        else {
            panic!("expected a thought");
        };
        assert!(*truncated);
        assert!(text.len() <= MAX_ITEM_TEXT_BYTES);
        assert!(!text.ends_with("more"));
    }

    #[test]
    fn selection_moves_within_bounds_and_resumes_following_at_the_end() {
        let mut model = model_with(sample_turn());
        let last = model.items.len() - 1;
        assert_eq!(model.selected_index(), Some(last));

        model.select_by(-2);
        assert_eq!(model.selected_index(), Some(last - 2));
        model.select_by(-100);
        assert_eq!(model.selected_index(), Some(0));
        model.select_by(100);
        assert_eq!(model.selected, None);
    }

    #[test]
    fn replayed_turns_are_marked() {
        let mut model = ActivityModel::default();
        model.apply_replayed(event(1, 0, ActivityKind::TurnStarted));
        model.apply(event(3, 0, ActivityKind::TurnStarted));

        let replayed = model
            .items
            .iter()
            .map(|item| matches!(item.kind, ItemKind::Turn { replayed: true, .. }))
            .collect::<Vec<_>>();
        assert_eq!(replayed, [true, false]);
    }

    #[test]
    fn dropped_turns_leave_the_earlier_ones_as_they_were() {
        let mut model = ActivityModel::default();
        model.apply(event(0, 0, ActivityKind::TurnStarted));
        model.apply(event(2, 0, ActivityKind::TurnStarted));
        model.apply(event(4, 0, ActivityKind::TurnStarted));

        model.drop_turns_from(2);
        model.apply(event(2, 0, ActivityKind::TurnStarted));
        let turns = model
            .items
            .iter()
            .filter_map(|item| match item.kind {
                ItemKind::Turn {
                    number, replayed, ..
                } => Some((item.turn_id, number, replayed)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(turns, [(0, 1, false), (2, 2, false)]);
    }

    #[test]
    fn raw_streams_are_kept_for_recent_rounds_within_a_byte_limit() {
        let mut model = ActivityModel::default();
        for round in 1..=RAW_ROUNDS + 2 {
            model.push_raw(1, round, "{\"type\":\"ping\"}".into());
        }
        assert_eq!(model.raw.len(), RAW_ROUNDS);
        assert!(model.raw_round(1, 1).is_none());
        assert!(model.raw_round(1, RAW_ROUNDS + 2).is_some());

        model.push_raw(2, 1, "x".repeat(MAX_RAW_BYTES_PER_ROUND));
        model.push_raw(2, 1, "y".into());
        let raw = model.raw_round(2, 1).unwrap();
        assert_eq!(raw.events.len(), 1);
        assert!(raw.truncated);
    }

    #[test]
    fn raw_view_opens_only_for_rounds_with_a_kept_stream() {
        let mut model = model_with(sample_turn());
        model.select_first();
        assert!(model.open_raw_view().is_err());

        model.selected = Some(1);
        assert_eq!(
            model.open_raw_view(),
            Err("The raw stream of this round is no longer kept")
        );
        model.push_raw(4, 1, "{}".into());
        assert!(model.open_raw_view().is_ok());
        assert_eq!(model.raw_view.as_ref().map(|v| v.round), Some(1));
    }

    #[test]
    fn derived_signals_follow_the_item_that_raised_them() {
        let mut model = ActivityModel::default();
        model.apply(event(1, 0, ActivityKind::TurnStarted));
        for _ in 0..3 {
            model.apply(event(
                1,
                1,
                ActivityKind::ToolStarted {
                    call_id: "c".into(),
                    name: "read_file".into(),
                    input_summary: "a.rs".into(),
                },
            ));
        }

        assert_eq!(model.phase(), Phase::Investigating);
        let last = &model.items.last().unwrap().kind;
        assert!(
            matches!(last, ItemKind::Signal(text) if text.contains("read_file a.rs started 3 times"))
        );
    }

    /// A main round whose `agent` call runs a subagent round of the same
    /// number: the subagent's events must not land on the main items.
    pub(crate) fn turn_with_subagent() -> Vec<ActivityEvent> {
        let sub = |round, kind| ActivityEvent {
            agent: Some("agent-1".into()),
            ..event(2, round, kind)
        };
        vec![
            event(2, 0, ActivityKind::TurnStarted),
            event(2, 1, ActivityKind::RoundStarted { model: "m".into() }),
            event(2, 1, ActivityKind::ThoughtStarted { block: 0 }),
            event(
                2,
                1,
                ActivityKind::ThoughtDelta {
                    block: 0,
                    text: "delegate the search".into(),
                },
            ),
            event(
                2,
                1,
                ActivityKind::ToolStarted {
                    call_id: "agent-1".into(),
                    name: "agent".into(),
                    input_summary: "Find the greeting".into(),
                },
            ),
            sub(1, ActivityKind::RoundStarted { model: "m".into() }),
            sub(1, ActivityKind::ThoughtStarted { block: 0 }),
            sub(
                1,
                ActivityKind::ThoughtDelta {
                    block: 0,
                    text: "look in a.txt".into(),
                },
            ),
            sub(
                1,
                ActivityKind::RoundFinished {
                    stop_reason: Some("end_turn".into()),
                    usage: None,
                    ttft_ms: None,
                    elapsed_ms: 40,
                },
            ),
            event(
                2,
                1,
                ActivityKind::ToolFinished {
                    call_id: "agent-1".into(),
                    name: "agent".into(),
                    is_error: false,
                    elapsed_ms: 50,
                    summary: "a.txt:1 says hello".into(),
                },
            ),
        ]
    }

    #[test]
    fn subagent_events_fill_their_own_items() {
        let mut model = ActivityModel::default();
        for event in turn_with_subagent() {
            model.apply(event);
        }

        let thoughts = model
            .items
            .iter()
            .filter_map(|item| match &item.kind {
                ItemKind::Thought { text, .. } => Some((item.agent.as_deref(), text.as_str())),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            thoughts,
            [
                (None, "delegate the search"),
                (Some("agent-1"), "look in a.txt")
            ]
        );
        let rounds = model
            .items
            .iter()
            .filter_map(|item| match &item.kind {
                ItemKind::Round { stats, .. } => Some((item.agent.as_deref(), stats.is_some())),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(rounds, [(None, false), (Some("agent-1"), true)]);
    }

    fn started(call_id: &str, name: &str, input_summary: &str) -> ActivityKind {
        ActivityKind::ToolStarted {
            call_id: call_id.into(),
            name: name.into(),
            input_summary: input_summary.into(),
        }
    }

    fn finished(call_id: &str, name: &str, summary: &str) -> ActivityKind {
        ActivityKind::ToolFinished {
            call_id: call_id.into(),
            name: name.into(),
            is_error: false,
            elapsed_ms: 5,
            summary: summary.into(),
        }
    }

    /// Two explorers of one batch whose events arrive interleaved, each
    /// using the call ID `call_0` for its own read.
    pub(crate) fn turn_with_concurrent_subagents() -> Vec<ActivityEvent> {
        let sub = |agent: &str, kind| ActivityEvent {
            agent: Some(agent.into()),
            ..event(2, 1, kind)
        };
        vec![
            event(2, 0, ActivityKind::TurnStarted),
            event(2, 1, ActivityKind::RoundStarted { model: "m".into() }),
            event(2, 1, started("agent-a", "agent", "Scan src")),
            event(2, 1, started("agent-b", "agent", "Scan docs")),
            sub("agent-a", ActivityKind::RoundStarted { model: "m".into() }),
            sub("agent-b", ActivityKind::RoundStarted { model: "m".into() }),
            sub("agent-a", started("call_0", "read_file", "src/lib.rs")),
            sub("agent-b", started("call_0", "read_file", "docs/guide.md")),
            sub("agent-b", finished("call_0", "read_file", "guide read")),
            sub("agent-a", finished("call_0", "read_file", "lib read")),
            event(2, 1, finished("agent-b", "agent", "docs done")),
            event(2, 1, finished("agent-a", "agent", "src done")),
        ]
    }

    #[test]
    fn subagents_sharing_a_call_id_keep_their_own_tool_items() {
        let model = model_with(turn_with_concurrent_subagents());

        let reads = model
            .items
            .iter()
            .filter_map(|item| match &item.kind {
                ItemKind::Tool(tool) if tool.name == "read_file" => Some((
                    item.agent.as_deref(),
                    tool.input_summary.as_str(),
                    tool.state.clone(),
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        let done = |summary: &str| ToolState::Finished {
            is_error: false,
            elapsed_ms: 5,
            summary: summary.into(),
        };
        assert_eq!(
            reads,
            [
                (Some("agent-a"), "src/lib.rs", done("lib read")),
                (Some("agent-b"), "docs/guide.md", done("guide read")),
            ]
        );
    }

    #[test]
    fn a_subagent_item_inserted_above_the_selection_keeps_it_selected() {
        let mut events = turn_with_concurrent_subagents();
        let late = events.split_off(7);
        let mut model = model_with(events);
        let scan_docs = model
            .items
            .iter()
            .position(
                |item| matches!(&item.kind, ItemKind::Tool(tool) if tool.call_id == "agent-b"),
            )
            .unwrap();
        model.selected = Some(scan_docs);

        for event in late {
            model.apply(event);
        }
        model.apply(ActivityEvent {
            agent: Some("agent-a".into()),
            ..event(2, 1, started("call_1", "search_text", "fn main"))
        });

        let selected = &model.items[model.selected_index().unwrap()];
        assert!(
            matches!(&selected.kind, ItemKind::Tool(tool) if tool.call_id == "agent-b"),
            "selection moved off the agent-b call"
        );
    }

    #[test]
    fn command_output_goes_to_the_running_command_not_the_newest_row() {
        let intent = |block, call_id: &str, name: &str| {
            event(
                2,
                1,
                ActivityKind::ToolIntentStarted {
                    block,
                    call_id: call_id.into(),
                    name: name.into(),
                },
            )
        };
        let mut model = model_with(vec![
            event(2, 0, ActivityKind::TurnStarted),
            intent(0, "agent-1", "agent"),
            intent(1, "main-test", "run_command"),
            event(2, 1, started("agent-1", "agent", "Fix the bug")),
            ActivityEvent {
                agent: Some("agent-1".into()),
                ..event(2, 1, started("call_0", "run_command", "cargo build"))
            },
        ]);

        model.attach_command_output(CommandOutputState {
            command: "cargo build".into(),
            exit_code: Some(0),
            timed_out: false,
            stdout_preview: "built".into(),
            stderr_preview: String::new(),
        });

        let outputs = model
            .items
            .iter()
            .filter_map(|item| match &item.kind {
                ItemKind::Tool(tool) if tool.name == "run_command" => {
                    Some((tool.call_id.as_str(), tool.command_output.is_some()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(outputs, [("call_0", true), ("main-test", false)]);
    }

    #[test]
    fn raw_view_is_refused_for_subagent_rounds() {
        let mut model = ActivityModel::default();
        for event in turn_with_subagent() {
            model.apply(event);
        }
        model.push_raw(2, 1, "{}".into());
        let subagent_round = model
            .items
            .iter()
            .position(|item| item.agent.is_some())
            .unwrap();

        model.selected = Some(subagent_round);
        assert_eq!(
            model.open_raw_view(),
            Err("Raw streams of subagent rounds are not kept")
        );
        model.selected = Some(1);
        assert!(model.open_raw_view().is_ok());
    }
}
