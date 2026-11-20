//! The plan from the conversation history, and the plan a running turn
//! changes. Both go through `Plan::apply`, so the live plan and the replay
//! cannot disagree unless the rules changed between versions, which
//! `replay` reports instead of hiding.

use super::render::{nudge, receipt};
use super::{Plan, PlanError, PlanUpdate, PlanView};
use crate::constants::{PLAN_NUDGE_ROUNDS, PLAN_TOOL_NAME};
use crate::conversation::{ContentBlock, Message, MessageContent};
use crate::tools::error::ToolError;
use serde_json::Value;
use std::collections::HashMap;

/// The plan `messages` leave behind.
pub(crate) struct Replayed {
    pub(crate) plan: Option<Plan>,
    /// Calls that succeeded when they ran but no longer apply. Empty unless
    /// `Plan::apply` became stricter in a later version.
    pub(crate) skipped: Vec<(String, PlanError)>,
}

/// Folds every `update_plan` call in `messages` whose saved result is not
/// an error, in history order.
///
/// Refused, cancelled, unparsable and unfinished calls (which
/// `repair_incomplete_tools` turns into errors) never changed the plan, so
/// they are skipped. Results cut to fit the window keep their `is_error`,
/// and tool inputs are never cut, so what the fold reads is whole.
///
/// Callers pass a prefix of the raw history: all of it for the current
/// plan, or up to a compaction cut for the plan at the cut.
pub(crate) fn replay(messages: &[Message]) -> Replayed {
    let blocks = || {
        messages.iter().flat_map(|message| match &message.content {
            MessageContent::Blocks(blocks) => blocks.as_slice(),
            MessageContent::Text(_) => &[],
        })
    };
    let succeeded: HashMap<&str, bool> = blocks()
        .filter_map(|block| match block {
            ContentBlock::ToolResult {
                tool_use_id,
                is_error,
                ..
            } => Some((tool_use_id.as_str(), *is_error != Some(true))),
            _ => None,
        })
        .collect();
    let mut plan = None;
    let mut skipped = Vec::new();
    for block in blocks() {
        let ContentBlock::ToolUse { id, name, input } = block else {
            continue;
        };
        if name != PLAN_TOOL_NAME || succeeded.get(id.as_str()) != Some(&true) {
            continue;
        }
        match PlanUpdate::parse(input).and_then(|update| Plan::apply(plan.as_ref(), &update)) {
            Ok(applied) => plan = Some(applied.plan),
            Err(reason) => skipped.push((id.clone(), reason)),
        }
    }
    Replayed { plan, skipped }
}

/// The main agent's plan during one turn: the replayed plan at turn start
/// plus the calls this turn applies, and the reminder policy.
///
/// Owned by `AgentService::submit_with_progress` for one turn and lent to
/// `run_agent_turn`. A subagent's turn gets none, so no code path lets a
/// subagent change the plan. Never saved: the tool results it produced are
/// the record.
pub(crate) struct PlanLedger {
    plan: Option<Plan>,
    /// Tool rounds of this turn since the last successful `update_plan`.
    rounds_without_update: usize,
    /// At most one reminder per turn.
    nudged: bool,
    skipped: Vec<(String, PlanError)>,
}

/// A successful call: the tool result text and the view for the Timeline.
pub(crate) struct Recorded {
    pub(crate) receipt: String,
    pub(crate) view: PlanView,
}

impl PlanLedger {
    /// The plan the saved history leaves, ready for a new turn.
    pub(crate) fn resume(history: &[Message]) -> Self {
        let Replayed { plan, skipped } = replay(history);
        Self {
            plan,
            rounds_without_update: 0,
            nudged: false,
            skipped,
        }
    }

    /// Applies one `update_plan` call. On failure nothing changes and the
    /// error goes back to the model as the tool result.
    pub(crate) fn record(&mut self, input: &Value) -> Result<Recorded, ToolError> {
        let applied = PlanUpdate::parse(input)
            .and_then(|update| Plan::apply(self.plan.as_ref(), &update))
            .map_err(|err| ToolError::Message(err.to_string()))?;
        let receipt = receipt(&applied);
        let view = applied.plan.view();
        self.plan = Some(applied.plan);
        self.rounds_without_update = 0;
        Ok(Recorded { receipt, view })
    }

    /// Called at the end of every round that ran tools. Returns the reminder
    /// and the round count when this round makes `PLAN_NUDGE_ROUNDS` rounds
    /// without an update, the plan has open work, and none was sent this
    /// turn. Without an open plan there is none, so one-step questions are
    /// not pushed into planning.
    pub(crate) fn round_finished(&mut self) -> Option<(String, usize)> {
        self.rounds_without_update += 1;
        let plan = self.plan.as_ref().filter(|plan| plan.has_open_work())?;
        if self.nudged || self.rounds_without_update < PLAN_NUDGE_ROUNDS {
            return None;
        }
        self.nudged = true;
        let rounds = self.rounds_without_update;
        Some((nudge(plan, rounds), rounds))
    }

    /// Replay problems found by `resume`, as warning lines, once.
    pub(crate) fn take_replay_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.skipped)
            .into_iter()
            .map(|(call_id, reason)| {
                format!("An earlier {PLAN_TOOL_NAME} call ({call_id}) no longer applies and is left out of the plan: {reason}")
            })
            .collect()
    }

    /// For the end-of-turn check that the live plan equals the replay.
    pub(crate) fn plan(&self) -> Option<&Plan> {
        self.plan.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use serde_json::json;

    fn call(id: &str, input: Value) -> Message {
        Message::assistant_blocks(vec![ContentBlock::ToolUse {
            id: id.into(),
            name: PLAN_TOOL_NAME.into(),
            input,
        }])
    }

    fn result(id: &str, ok: bool) -> Message {
        Message::user_blocks(vec![ContentBlock::ToolResult {
            tool_use_id: id.into(),
            content: "receipt".into(),
            is_error: (!ok).then_some(true),
        }])
    }

    fn start() -> Value {
        json!({"goal": "g", "tasks": [{"id": "a", "title": "A"}, {"id": "b", "title": "B"}]})
    }

    fn finish(id: &str) -> Value {
        json!({"tasks": [{"id": id, "status": "done", "note": "ok"}]})
    }

    #[test]
    fn only_calls_with_a_successful_saved_result_count() {
        let history = vec![
            Message::user_text("plan it".into()),
            call("1", start()),
            result("1", true),
            call("2", finish("a")),
            result("2", false),
            // No result: a crash before the result was saved.
            call("3", finish("b")),
        ];
        let view = replay(&history).plan.unwrap().view();
        assert_eq!(view.revision, 1);
        assert_eq!(view.counts.done, 0);
    }

    #[test]
    fn the_ledger_and_the_replay_agree() {
        let mut ledger = PlanLedger::resume(&[]);
        let mut history = Vec::new();
        for (id, input) in [
            ("1", start()),
            ("2", finish("a")),
            ("3", json!({"tasks": [{"id": "zz"}]})),
        ] {
            let ok = ledger.record(&input).is_ok();
            history.extend([call(id, input), result(id, ok)]);
        }
        assert_eq!(ledger.plan(), replay(&history).plan.as_ref());
        assert!(replay(&history).skipped.is_empty());
    }

    #[test]
    fn the_reminder_comes_once_after_rounds_without_an_update_with_open_work() {
        let mut ledger = PlanLedger::resume(&[]);
        for _ in 0..PLAN_NUDGE_ROUNDS {
            assert!(ledger.round_finished().is_none(), "no plan, no reminder");
        }
        ledger.record(&start()).unwrap();
        for _ in 1..PLAN_NUDGE_ROUNDS {
            assert!(ledger.round_finished().is_none());
        }
        let (text, rounds) = ledger.round_finished().unwrap();
        assert_eq!(rounds, PLAN_NUDGE_ROUNDS);
        assert!(text.contains("2 tasks open"), "{text}");
        for _ in 0..PLAN_NUDGE_ROUNDS {
            assert!(ledger.round_finished().is_none(), "once per turn");
        }
    }

    #[test]
    fn a_finished_plan_gets_no_reminder() {
        let mut ledger = PlanLedger::resume(&[]);
        ledger
            .record(&json!({"goal": "g", "tasks": [{"id": "a", "title": "A", "status": "done"}]}))
            .unwrap();
        for _ in 0..=PLAN_NUDGE_ROUNDS {
            assert!(ledger.round_finished().is_none());
        }
    }

    proptest! {
        /// Every prefix of a history replays to the plan the ledger had after
        /// the same calls: rewind (a shorter history), a compaction cut and
        /// a live turn all see one plan.
        #[test]
        fn every_prefix_replays_to_the_ledger_plan(
            steps in prop::collection::vec((0usize..4, 0usize..3, any::<bool>()), 1..16)
        ) {
            let ids = ["a", "b", "c"];
            let mut ledger = PlanLedger::resume(&[]);
            let mut history = vec![Message::user_text("go".into())];
            let mut expected = vec![(history.len(), None)];
            for (n, (kind, task, refused)) in steps.into_iter().enumerate() {
                let input = match kind {
                    0 => start(),
                    1 => finish(ids[task]),
                    2 => json!({"tasks": [{"id": ids[task], "done": [1, 3], "note": "some"}]}),
                    _ => json!({"tasks": [{"id": ids[task], "title": "C"}]}),
                };
                let id = n.to_string();
                // A refused or cancelled call leaves the plan as it was.
                let ok = !refused && ledger.record(&input).is_ok();
                history.extend([call(&id, input), result(&id, ok)]);
                expected.push((history.len(), ledger.plan().cloned()));
            }
            for (len, plan) in expected {
                prop_assert_eq!(replay(&history[..len]).plan, plan);
            }
        }
    }
}
