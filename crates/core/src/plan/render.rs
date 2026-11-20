//! Everything the model reads about the plan: the tool definition, the
//! receipt each call returns, the plan block in a compaction summary, and
//! the reminder. Kept in one place so the TUI never formats model-facing
//! text.
//!
//! Nothing here goes into the system prompt or into a message that was
//! already sent, because the prompt cache relies on the history only
//! growing: receipts and reminders are new tool-result text, and the
//! summary block changes only when the summary itself does.

use super::{
    Applied, MAX_BATCH, MAX_GOAL_CHARS, MAX_NOTE_CHARS, MAX_TITLE_CHARS, MAX_UNITS, MAX_WEIGHT,
};
use super::{Plan, PlanRow, PlanView, TaskStatus};
use crate::constants::PLAN_TOOL_NAME;
use crate::conversation::ToolDefinition;
use serde_json::{Value, json};

/// Open tasks a receipt lists before "+N more".
const RECEIPT_OPEN_TASKS: usize = 12;
/// Notes in receipts and the summary block are cut to this.
const NOTE_PREVIEW_CHARS: usize = 120;
/// Task changes a one-line input summary names before "+N".
const SUMMARY_PATCHES: usize = 3;

pub(crate) fn tool_definition() -> ToolDefinition {
    let statuses = TaskStatus::ALL.map(TaskStatus::label);
    ToolDefinition {
        name: PLAN_TOOL_NAME.to_string(),
        description: "Record the plan for work with several steps, and your progress on it. The user follows this plan in the Timeline view. Each call applies all of its task changes or none of them; tasks and fields you leave out stay as they are. The result lists the open tasks.".to_string(),
        input_schema: json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["tasks"],
            "properties": {
                "goal": {
                    "type": "string",
                    "description": format!("Only to start a new plan: the goal in a few words, at most {MAX_GOAL_CHARS} characters. Replaces the current plan; sending the current goal again keeps it.")
                },
                "tasks": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": MAX_BATCH,
                    "items": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["id"],
                        "properties": {
                            "id": {"type": "string", "description": "Stable short ID such as parse-args; reuse it to change the task."},
                            "parent": {"type": ["string", "null"], "description": "ID of the task this one is part of; null moves it to the top level. Leave out to keep it."},
                            "title": {"type": "string", "description": format!("Required for a new task: the outcome in a few words, at most {MAX_TITLE_CHARS} characters.")},
                            "status": {"type": "string", "enum": statuses, "description": "For a task without subtasks. A task with subtasks takes its status from them and accepts only dropped, which drops its subtasks too."},
                            "done": {"type": "array", "items": {"type": "integer", "minimum": 0, "maximum": MAX_UNITS}, "minItems": 2, "maxItems": 2, "description": "[completed, total] units for a task without subtasks, such as [3, 7] for 3 of 7 failing tests fixed. Without status, it sets pending, active or done."},
                            "weight": {"type": "integer", "minimum": 1, "maximum": MAX_WEIGHT, "description": "Effort relative to sibling tasks; default 1. A task split into subtasks keeps its weight."},
                            "note": {"type": "string", "description": format!("What changed and why, at most {MAX_NOTE_CHARS} characters. Required when an existing task's status, done, parent or weight changes, and for a blocked or dropped task.")}
                        }
                    }
                }
            }
        }),
    }
}

/// `Plan r5 "Add a Timeline": 32% (2 of 7 tasks done), 1 active, 1 blocked`
pub(crate) fn headline(view: &PlanView) -> String {
    let counts = view.counts;
    let mut line = format!(
        "Plan r{} \"{}\": {}% ({} of {} tasks done)",
        view.revision,
        view.goal,
        view.progress.done / 10,
        counts.done,
        counts.total()
    );
    for (count, label) in [(counts.active, "active"), (counts.blocked, "blocked")] {
        if count > 0 {
            line.push_str(&format!(", {count} {label}"));
        }
    }
    line
}

/// The tool result of a successful call: the headline, what changed, the
/// open tasks in tree order (dsh's `timeline_pull`), and a closing line
/// when no task is active or every task is done.
///
/// The first line is the headline, which Activity and the transcript show.
/// No line starts with "Changed: ", which the transcript reads as an edit
/// diff.
pub(crate) fn receipt(applied: &Applied) -> String {
    let view = applied.plan.view();
    let mut out = headline(&view);
    if applied.changes.is_empty() {
        out.push_str(" (unchanged)");
    } else {
        out.push_str(&format!("\nUpdated: {}", applied.changes.join("; ")));
    }
    let open = open_rows(&view);
    if !open.is_empty() {
        out.push_str("\nOpen tasks, in order:");
        for row in open.iter().take(RECEIPT_OPEN_TASKS) {
            out.push_str(&format!("\n* {}", task_line(&view, row)));
        }
        if open.len() > RECEIPT_OPEN_TASKS {
            out.push_str(&format!("\n+{} more", open.len() - RECEIPT_OPEN_TASKS));
        }
    }
    if open.is_empty() {
        out.push_str("\nAll tasks are done. Check that the goal is met before your final answer.");
    } else if view.counts.active == 0 {
        out.push_str("\nNo task is active; mark the one you work on next as active.");
    }
    out
}

/// The plan as it stood at a compaction cut, for the end of the summary
/// message. The same plan renders the same bytes, so the summary message
/// stays cacheable. `None` for a finished plan, which the model no longer
/// needs.
pub(crate) fn summary_block(plan: &Plan) -> Option<String> {
    if !plan.has_open_work() {
        return None;
    }
    let view = plan.view();
    let mut out = format!(
        "The plan at this point (change it with update_plan; task IDs are stable):\n{}",
        headline(&view)
    );
    for row in view
        .rows
        .iter()
        .filter(|row| row.status != TaskStatus::Dropped)
    {
        out.push('\n');
        out.push_str(&"  ".repeat(row.depth as usize));
        out.push_str("* ");
        out.push_str(&row_text(row));
    }
    Some(out)
}

/// The reminder appended to a round's last tool result.
pub(crate) fn nudge(plan: &Plan, rounds: usize) -> String {
    let open = open_rows(&plan.view()).len();
    format!(
        "\n\n(Plan check: {rounds} rounds since the last {PLAN_TOOL_NAME}, with {open} tasks open. Record what you finished, or revise the plan if the work changed.)"
    )
}

/// One line for a call, such as `goal: Add a Timeline` or
/// `types done, fold 1/4 +2`. Reads the raw input leniently, so a call that
/// fails to parse still gets a summary.
pub(crate) fn summarize_update(input: &Value) -> String {
    if let Some(goal) = input["goal"].as_str() {
        return format!("goal: {goal}");
    }
    let Some(tasks) = input["tasks"].as_array() else {
        return String::new();
    };
    let mut parts = tasks
        .iter()
        .take(SUMMARY_PATCHES)
        .filter_map(|task| {
            let id = task["id"].as_str()?;
            let state = if let Some(status) = task["status"].as_str() {
                status.to_string()
            } else if let Some([completed, total]) = task["done"]
                .as_array()
                .and_then(|done| <&[Value; 2]>::try_from(done.as_slice()).ok())
            {
                format!("{}/{}", completed, total)
            } else if task.get("title").is_some() {
                "added".to_string()
            } else {
                "edited".to_string()
            };
            Some(format!("{id} {state}"))
        })
        .collect::<Vec<_>>()
        .join(", ");
    if tasks.len() > SUMMARY_PATCHES {
        parts.push_str(&format!(" +{}", tasks.len() - SUMMARY_PATCHES));
    }
    parts
}

/// Open leaves in tree order.
fn open_rows(view: &PlanView) -> Vec<&PlanRow> {
    view.rows
        .iter()
        .filter(|row| !row.is_group && row.status.is_open())
        .collect()
}

/// `fold (in core) [active 1/4] Rebuild the plan: replay skips errors`
fn task_line(view: &PlanView, row: &PlanRow) -> String {
    let parent = view
        .rows
        .iter()
        .take_while(|other| other.id != row.id)
        .filter(|other| other.depth + 1 == row.depth)
        .last()
        .map(|parent| format!(" (in {})", parent.id))
        .unwrap_or_default();
    let text = row_text(row);
    match text.split_once(' ') {
        Some((id, rest)) => format!("{id}{parent} {rest}"),
        None => text,
    }
}

/// `fold [active 1/4] Rebuild the plan: replay skips errors`
fn row_text(row: &PlanRow) -> String {
    let state = match row.done {
        Some([completed, total]) if total > 1 && row.status != TaskStatus::Done => {
            format!("{} {completed}/{total}", row.status.label())
        }
        _ => row.status.label().to_string(),
    };
    let mut text = format!("{} [{state}] {}", row.id, row.title);
    if !row.note.is_empty() && row.status != TaskStatus::Pending {
        text.push_str(": ");
        text.push_str(&cut(&row.note, NOTE_PREVIEW_CHARS));
    }
    text
}

fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut cut = text.chars().take(max).collect::<String>();
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::tests::apply;

    fn plan() -> Plan {
        let plan = apply(
            None,
            json!({"goal": "Add a Timeline", "tasks": [
                {"id": "core", "title": "Plan model in core", "weight": 3},
                {"id": "types", "parent": "core", "title": "Types", "status": "active"},
                {"id": "fold", "parent": "core", "title": "Fold", "done": [0, 4]},
                {"id": "adr", "title": "ADR 18"}
            ]}),
        )
        .unwrap()
        .plan;
        apply(
            Some(&plan),
            json!({"tasks": [
                {"id": "types", "status": "done", "note": "14 tests pass"},
                {"id": "fold", "done": [1, 4], "note": "replay skips errored calls"}
            ]}),
        )
        .unwrap()
        .plan
    }

    #[test]
    fn a_receipt_leads_with_the_headline_and_lists_open_tasks() {
        let applied = apply(
            Some(&plan()),
            json!({"tasks": [{"id": "adr", "status": "blocked", "note": "waits on review"}]}),
        )
        .unwrap();
        assert_eq!(
            receipt(&applied),
            "Plan r3 \"Add a Timeline\": 46% (1 of 3 tasks done), 1 active, 1 blocked\n\
             Updated: adr blocked (was pending)\n\
             Open tasks, in order:\n\
             * fold (in core) [active 1/4] Fold: replay skips errored calls\n\
             * adr [blocked] ADR 18: waits on review"
        );
    }

    #[test]
    fn a_receipt_asks_for_an_active_task_and_a_final_check() {
        let idle = apply(
            Some(&plan()),
            json!({"tasks": [{"id": "fold", "status": "pending", "done": [0, 4], "note": "later"}]}),
        )
        .unwrap();
        assert!(
            receipt(&idle).ends_with("No task is active; mark the one you work on next as active.")
        );
        let finished = apply(
            Some(&plan()),
            json!({"tasks": [
                {"id": "fold", "status": "done", "note": "all"},
                {"id": "adr", "status": "done", "note": "merged"}
            ]}),
        )
        .unwrap();
        assert!(
            receipt(&finished).ends_with("Check that the goal is met before your final answer.")
        );
        assert!(summary_block(&finished.plan).is_none());
    }

    #[test]
    fn receipts_never_look_like_an_edit_diff() {
        let applied = apply(
            Some(&plan()),
            json!({"tasks": [{"id": "x", "title": "Changed: a.rs"}]}),
        )
        .unwrap();
        let text = receipt(&applied);
        // `transcript::diff_preview` starts a diff at this prefix.
        assert!(!text.contains("\nChanged: "), "{text}");
    }

    #[test]
    fn the_summary_block_shows_the_tree_and_is_stable() {
        let plan = plan();
        let block = summary_block(&plan).unwrap();
        assert_eq!(
            block,
            "The plan at this point (change it with update_plan; task IDs are stable):\n\
             Plan r2 \"Add a Timeline\": 46% (1 of 3 tasks done), 1 active\n\
             * core [active] Plan model in core\n\
             \x20 * types [done] Types: 14 tests pass\n\
             \x20 * fold [active 1/4] Fold: replay skips errored calls\n\
             * adr [pending] ADR 18"
        );
        assert_eq!(summary_block(&plan).unwrap(), block);
    }

    #[test]
    fn input_summaries_name_goals_and_changes() {
        assert_eq!(
            summarize_update(&json!({"goal": "Ship it", "tasks": []})),
            "goal: Ship it"
        );
        assert_eq!(
            summarize_update(&json!({"tasks": [
                {"id": "a", "status": "done"}, {"id": "b", "done": [1, 4]},
                {"id": "c", "title": "C"}, {"id": "d", "note": "x"}
            ]})),
            "a done, b 1/4, c added +1"
        );
        assert_eq!(summarize_update(&json!("nonsense")), "");
    }

    #[test]
    fn the_schema_lists_every_status() {
        let definition = tool_definition();
        let listed = &definition.input_schema["properties"]["tasks"]["items"]["properties"]["status"]
            ["enum"];
        assert_eq!(
            listed,
            &json!(["pending", "active", "blocked", "done", "dropped"])
        );
    }
}
