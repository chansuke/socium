//! Timeline view state: the latest `PlanView` from core, the selected task,
//! and whose history is open. The TUI derives nothing about the plan; it
//! replaces the view whenever core sends one and never writes back.

use socium_core::plan::{PlanRow, PlanView, TaskStatus};
use std::collections::BTreeSet;

#[derive(Debug, Default)]
pub(crate) struct TimelineModel {
    plan: Option<PlanView>,
    /// The selected task by ID, not by row: updates add and move rows, and
    /// an index would jump to another task. `None` follows the task the
    /// agent is on.
    selected: Option<String>,
    /// Tasks whose history rows are shown.
    expanded: BTreeSet<String>,
}

impl TimelineModel {
    /// Replaces the plan, with each `ActivityKind::PlanUpdated` and with
    /// `AgentService::plan()` whenever the history may have changed under
    /// the view: startup, /resume, /rewind, /clear, and when a turn ends.
    pub(crate) fn show(&mut self, plan: Option<PlanView>) {
        let same_goal = matches!((&self.plan, &plan), (Some(a), Some(b)) if a.goal == b.goal);
        if same_goal {
            let plan = plan.as_ref().expect("matched above");
            let exists = |id: &String| plan.rows.iter().any(|row| &row.id == id);
            self.selected = self.selected.take().filter(exists);
            self.expanded.retain(exists);
        } else {
            self.selected = None;
            self.expanded.clear();
        }
        self.plan = plan;
    }

    pub(crate) fn plan(&self) -> Option<&PlanView> {
        self.plan.as_ref()
    }

    /// `plan 32% 2/7` for other titles; `None` without a plan.
    pub(crate) fn chip(&self) -> Option<String> {
        let plan = self.plan.as_ref()?;
        Some(format!(
            "plan {}% {}/{}",
            plan.progress.done / 10,
            plan.counts.done,
            plan.counts.total()
        ))
    }

    /// Rows a view lists: dropped tasks are hidden.
    pub(crate) fn visible_rows(&self) -> Vec<&PlanRow> {
        self.plan
            .iter()
            .flat_map(|plan| &plan.rows)
            .filter(|row| row.status != TaskStatus::Dropped)
            .collect()
    }

    /// The row kept in view when nothing is selected: the first active
    /// leaf, else the first blocked one, else the first pending one, else
    /// the last row. The Timeline follows the agent the way Activity follows
    /// the newest item.
    pub(crate) fn follow_target(&self) -> Option<&PlanRow> {
        let rows = self.visible_rows();
        let leaf = |status| {
            rows.iter()
                .find(|row| !row.is_group && row.status == status)
                .copied()
        };
        leaf(TaskStatus::Active)
            .or_else(|| leaf(TaskStatus::Blocked))
            .or_else(|| leaf(TaskStatus::Pending))
            .or_else(|| rows.last().copied())
    }

    /// The selected row, or the followed one.
    pub(crate) fn current(&self) -> Option<&PlanRow> {
        match &self.selected {
            Some(id) => self.visible_rows().into_iter().find(|row| &row.id == id),
            None => self.follow_target(),
        }
    }

    pub(crate) fn is_selected(&self, id: &str) -> bool {
        self.current().is_some_and(|row| row.id == id)
    }

    pub(crate) fn is_expanded(&self, id: &str) -> bool {
        self.expanded.contains(id)
    }

    /// Moves the selection by `delta` visible rows.
    pub(crate) fn select_by(&mut self, delta: isize) {
        let rows = self.visible_rows();
        if rows.is_empty() {
            return;
        }
        let current = self
            .current()
            .and_then(|row| rows.iter().position(|other| other.id == row.id))
            .unwrap_or(0);
        let next = current.saturating_add_signed(delta).min(rows.len() - 1);
        self.selected = Some(rows[next].id.clone());
    }

    pub(crate) fn select_first(&mut self) {
        self.selected = self.visible_rows().first().map(|row| row.id.clone());
    }

    pub(crate) fn select_last(&mut self) {
        self.selected = self.visible_rows().last().map(|row| row.id.clone());
    }

    /// Enter: shows or hides the current task's history.
    pub(crate) fn toggle_history(&mut self) {
        let Some(id) = self.current().map(|row| row.id.clone()) else {
            return;
        };
        if !self.expanded.remove(&id) {
            self.expanded.insert(id);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use socium_core::plan::{LeafCounts, Progress, RevisionView};

    pub(crate) fn row(id: &str, depth: u8, status: TaskStatus, is_group: bool) -> PlanRow {
        PlanRow {
            id: id.into(),
            depth,
            title: format!("Task {id}"),
            status,
            is_group,
            done: (!is_group).then_some([u16::from(status == TaskStatus::Done), 1]),
            progress: if status == TaskStatus::Done { 1000 } else { 0 },
            weight: 1,
            note: String::new(),
            history: vec![RevisionView {
                revision: 1,
                status,
                done: None,
                note: "added".into(),
            }],
        }
    }

    pub(crate) fn sample_plan() -> PlanView {
        PlanView {
            goal: "Add a Timeline".into(),
            revision: 5,
            progress: Progress {
                done: 320,
                active: 180,
                pending: 400,
                blocked: 100,
            },
            counts: LeafCounts {
                done: 2,
                active: 1,
                pending: 3,
                blocked: 1,
            },
            rows: vec![
                row("core", 0, TaskStatus::Active, true),
                row("types", 1, TaskStatus::Done, false),
                row("fold", 1, TaskStatus::Active, false),
                row("old", 1, TaskStatus::Dropped, false),
                row("tui", 0, TaskStatus::Pending, true),
                row("slot", 1, TaskStatus::Pending, false),
                row("view", 1, TaskStatus::Pending, false),
                row("headless", 0, TaskStatus::Blocked, false),
                row("adr", 0, TaskStatus::Done, false),
            ],
        }
    }

    #[test]
    fn it_follows_the_active_task_until_a_task_is_selected() {
        let mut model = TimelineModel::default();
        model.show(Some(sample_plan()));
        assert_eq!(model.current().unwrap().id, "fold");
        assert_eq!(model.chip().unwrap(), "plan 32% 2/7");
        model.select_by(1);
        assert_eq!(
            model.current().unwrap().id,
            "tui",
            "dropped rows are skipped"
        );
    }

    #[test]
    fn the_selection_stays_on_its_task_and_a_new_goal_clears_it() {
        let mut model = TimelineModel::default();
        model.show(Some(sample_plan()));
        model.select_first();
        model.select_by(5);
        model.toggle_history();
        assert_eq!(model.current().unwrap().id, "view");

        let mut moved = sample_plan();
        moved
            .rows
            .insert(0, row("new", 0, TaskStatus::Pending, false));
        model.show(Some(moved.clone()));
        assert_eq!(model.current().unwrap().id, "view");
        assert!(model.is_expanded("view"));

        moved.goal = "Something else".into();
        model.show(Some(moved));
        assert_eq!(model.current().unwrap().id, "fold");
        assert!(!model.is_expanded("view"));
        model.show(None);
        assert!(model.current().is_none() && model.chip().is_none());
    }
}
