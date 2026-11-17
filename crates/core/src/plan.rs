//! The agent's plan for multi-step work: a tree of tasks with weighted
//! progress that only the main agent changes, through `update_plan`.
//!
//! The plan has no storage of its own. It is a fold over the successful
//! `update_plan` calls in the conversation history (see `replay`), so
//! rewind, resume, /clear, compaction and model switches keep it right
//! without code of their own. Nothing here reads a clock or does I/O: the
//! same history always yields the same plan.
//!
//! Public: the read model (`PlanView` and its parts) that the TUI and
//! headless output receive. Everything that can change a plan is private
//! to the crate.

mod render;
mod replay;

pub(crate) use render::{summarize_update, summary_block, tool_definition};
pub(crate) use replay::{PlanLedger, replay};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};

/// Tasks a plan may hold, dropped ones included, so its view stays small
/// enough to ride on one activity event.
const MAX_TASKS: usize = 40;
/// Task changes one call may carry.
const MAX_BATCH: usize = 30;
/// Deepest nesting: depth 0 is a top-level task.
const MAX_DEPTH: usize = 3;
/// Changes kept per task for the Timeline's history rows.
const HISTORY_PER_TASK: usize = 4;
const MAX_GOAL_CHARS: usize = 100;
const MAX_TITLE_CHARS: usize = 100;
const MAX_NOTE_CHARS: usize = 200;
const MAX_ID_CHARS: usize = 40;
const MAX_UNITS: u16 = 1_000;
const MAX_WEIGHT: u8 = 100;
const FULL: u16 = 1_000;

// ---------------------------------------------------------------------------
// Read model. Derived on demand, never stored. Integers only, so
// `ActivityKind` keeps deriving `Eq`.
// ---------------------------------------------------------------------------

/// What the Timeline view and headless consumers see of the current plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanView {
    pub goal: String,
    /// Changes applied since this goal started, from 1. Shown as `r5`.
    pub revision: u32,
    /// The whole plan's bar; the four shares sum to 1000.
    pub progress: Progress,
    pub counts: LeafCounts,
    /// Tree order: each task follows its parent, siblings in the order they
    /// were added. Dropped tasks are included; views may hide them.
    pub rows: Vec<PlanRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanRow {
    pub id: String,
    /// 0 for a top-level task.
    pub depth: u8,
    pub title: String,
    /// A group's status comes from its children; a leaf's is its own.
    pub status: TaskStatus,
    pub is_group: bool,
    /// `[completed, total]` for a leaf; `None` for a group.
    pub done: Option<[u16; 2]>,
    /// Completion of this task alone, in permille. Below 1000 until every
    /// leaf under it is done.
    pub progress: u16,
    pub weight: u8,
    /// The latest note; empty when none was given.
    pub note: String,
    /// The latest changes, oldest first, at most `HISTORY_PER_TASK`.
    pub history: Vec<RevisionView>,
}

/// One change to a task: the state it left the task in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisionView {
    pub revision: u32,
    pub status: TaskStatus,
    pub done: Option<[u16; 2]>,
    /// The note given with this change, or "added" for a new task.
    pub note: String,
}

/// Shares of the work by state, in permille, summing to 1000. `done` is
/// the completion shown as a percentage.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    pub done: u16,
    pub active: u16,
    pub pending: u16,
    pub blocked: u16,
}

/// Leaves by status; dropped leaves are not counted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeafCounts {
    pub done: u16,
    pub active: u16,
    pub pending: u16,
    pub blocked: u16,
}

impl LeafCounts {
    pub fn total(&self) -> u16 {
        self.done + self.active + self.pending + self.blocked
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Active,
    Blocked,
    Done,
    Dropped,
}

impl TaskStatus {
    /// The names the tool schema lists.
    pub(crate) const ALL: [Self; 5] = [
        Self::Pending,
        Self::Active,
        Self::Blocked,
        Self::Done,
        Self::Dropped,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Active => "active",
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Dropped => "dropped",
        }
    }

    fn is_open(self) -> bool {
        matches!(self, Self::Pending | Self::Active | Self::Blocked)
    }
}

// ---------------------------------------------------------------------------
// Domain. Values enter only through `PlanUpdate::parse`, so every field
// here already passed the boundary checks.
// ---------------------------------------------------------------------------

/// `completed` of `total` units: 0 <= completed <= total, 1 <= total <= MAX_UNITS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Units {
    completed: u16,
    total: u16,
}

impl Units {
    const ONE: Self = Self {
        completed: 0,
        total: 1,
    };

    fn pair(self) -> [u16; 2] {
        [self.completed, self.total]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Revision {
    revision: u32,
    status: TaskStatus,
    units: Option<Units>,
    note: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Task {
    id: String,
    parent: Option<String>,
    title: String,
    weight: u8,
    note: String,
    /// A leaf's own state. Ignored while the task has live children, and
    /// reset to pending `[0, 1]` when it gains them.
    status: TaskStatus,
    units: Units,
    /// Newest last, at most `HISTORY_PER_TASK`.
    history: VecDeque<Revision>,
}

/// The current plan. Invariants, checked by `apply` before it returns:
/// IDs are unique; every parent exists; no cycles; depth <= MAX_DEPTH; at
/// most MAX_TASKS tasks; no live task under a dropped one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Plan {
    goal: String,
    revision: u32,
    /// Insertion order, which is also the order of siblings on screen.
    tasks: Vec<Task>,
}

/// One `update_plan` call, parsed and checked field by field. Whether it
/// fits the current plan is `Plan::apply`'s question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlanUpdate {
    /// Starts a new plan, unless it equals the current goal: then the
    /// update applies to the current plan, so a retried start is harmless.
    goal: Option<String>,
    /// 1..=MAX_BATCH, IDs unique within the call, applied in order.
    patches: Vec<TaskPatch>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TaskPatch {
    id: String,
    /// `None` keeps the parent; `Some(None)` moves the task to the top.
    parent: Option<Option<String>>,
    title: Option<String>,
    status: Option<TaskStatus>,
    done: Option<Units>,
    weight: Option<u8>,
    note: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawUpdate {
    #[serde(default)]
    goal: Option<String>,
    tasks: Vec<RawPatch>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPatch {
    id: String,
    /// Absent keeps the parent, `null` moves the task to the top level.
    #[serde(default, deserialize_with = "present")]
    parent: Option<Option<String>>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    status: Option<TaskStatus>,
    #[serde(default)]
    done: Option<[i64; 2]>,
    #[serde(default)]
    weight: Option<i64>,
    #[serde(default)]
    note: Option<String>,
}

/// Tells a field sent as `null` from one left out.
fn present<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

impl PlanUpdate {
    /// The boundary: tool input to domain values. Unknown fields, wrong
    /// types, bad IDs, over-long text, `done` with completed > total,
    /// duplicate IDs and an empty or oversized batch are refused here.
    pub(crate) fn parse(input: &Value) -> Result<Self, PlanError> {
        let raw: RawUpdate = serde_json::from_value(input.clone()).map_err(|err| {
            PlanError::Input(format!("The input does not fit update_plan: {err}"))
        })?;
        let goal = raw
            .goal
            .map(|goal| line(&goal, MAX_GOAL_CHARS, false))
            .transpose()
            .map_err(|reason| PlanError::Input(format!("goal: {reason}")))?;
        if raw.tasks.is_empty() || raw.tasks.len() > MAX_BATCH {
            return Err(PlanError::Input(format!(
                "tasks must list 1 to {MAX_BATCH} task changes"
            )));
        }
        let mut seen = HashSet::new();
        let mut patches = Vec::with_capacity(raw.tasks.len());
        for (index, raw) in raw.tasks.into_iter().enumerate() {
            let field = |field: &'static str| {
                move |reason: String| PlanError::Field {
                    index,
                    field,
                    reason,
                }
            };
            let id = task_id(&raw.id).map_err(field("id"))?;
            if !seen.insert(id.clone()) {
                return Err(PlanError::DuplicateId { id });
            }
            let parent = match raw.parent {
                Some(Some(parent)) => Some(Some(task_id(&parent).map_err(field("parent"))?)),
                other => other.map(|_| None),
            };
            let title = raw
                .title
                .map(|title| line(&title, MAX_TITLE_CHARS, false))
                .transpose()
                .map_err(field("title"))?;
            let note = raw
                .note
                .map(|note| line(&note, MAX_NOTE_CHARS, true))
                .transpose()
                .map_err(field("note"))?;
            let done = raw
                .done
                .map(|[completed, total]| units(completed, total))
                .transpose()
                .map_err(field("done"))?;
            let weight = raw
                .weight
                .map(|weight| {
                    u8::try_from(weight)
                        .ok()
                        .filter(|weight| (1..=MAX_WEIGHT).contains(weight))
                        .ok_or_else(|| format!("must be from 1 to {MAX_WEIGHT}"))
                })
                .transpose()
                .map_err(field("weight"))?;
            patches.push(TaskPatch {
                id,
                parent,
                title,
                status: raw.status,
                done,
                weight,
                note: note.filter(|note| !note.is_empty()),
            });
        }
        Ok(Self { goal, patches })
    }
}

fn task_id(text: &str) -> Result<String, String> {
    let valid = (1..=MAX_ID_CHARS).contains(&text.chars().count())
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'));
    if valid {
        Ok(text.to_string())
    } else {
        Err(format!(
            "use 1 to {MAX_ID_CHARS} letters, digits, '.', '_', '/' or '-', such as parse-args"
        ))
    }
}

/// Trimmed text on one line, at most `max` characters.
fn line(text: &str, max: usize, allow_empty: bool) -> Result<String, String> {
    let text = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>();
    let text = text.trim();
    if text.is_empty() && !allow_empty {
        return Err("must not be empty".into());
    }
    if text.chars().count() > max {
        return Err(format!("must be at most {max} characters"));
    }
    Ok(text.to_string())
}

fn units(completed: i64, total: i64) -> Result<Units, String> {
    let max = i64::from(MAX_UNITS);
    if !(1..=max).contains(&total) || !(0..=total).contains(&completed) {
        return Err(format!(
            "must be [completed, total] with 0 <= completed <= total and 1 <= total <= {MAX_UNITS}"
        ));
    }
    Ok(Units {
        completed: completed as u16,
        total: total as u16,
    })
}

/// What one applied update did, for the receipt.
pub(crate) struct Applied {
    pub(crate) plan: Plan,
    /// One short line per changed task, in patch order. Empty when the
    /// update changed nothing; the revision then stays.
    pub(crate) changes: Vec<String>,
}

impl Plan {
    /// Applies `update` to `current` as one change: the whole update or none
    /// of it. Pure, so a refused update leaves nothing behind and the same
    /// inputs always give the same plan, which replay depends on. Applying
    /// an update to the plan it produced reports no change.
    pub(crate) fn apply(current: Option<&Plan>, update: &PlanUpdate) -> Result<Applied, PlanError> {
        let (base, started) = match (current, &update.goal) {
            (Some(plan), Some(goal)) if &plan.goal == goal => (plan.clone(), false),
            (_, Some(goal)) => (
                Plan {
                    goal: goal.clone(),
                    revision: 0,
                    tasks: Vec::new(),
                },
                true,
            ),
            (Some(plan), None) => (plan.clone(), false),
            (None, None) => return Err(PlanError::NoPlan),
        };
        let revision = base.revision + 1;
        let mut next = base.clone();
        for patch in &update.patches {
            next.merge(patch)?;
        }
        next.check_tree()?;
        let groups = next.group_ids();
        for patch in &update.patches {
            let gives_state = patch.done.is_some()
                || patch
                    .status
                    .is_some_and(|status| status != TaskStatus::Dropped);
            if gives_state && groups.contains(&patch.id) {
                return Err(PlanError::GroupState {
                    id: patch.id.clone(),
                    children: next.children_of(&patch.id).join(", "),
                });
            }
        }
        next.drop_subtrees(update);
        next.check_dropped_parents()?;
        let groups = next.group_ids();
        for task in &mut next.tasks {
            if groups.contains(&task.id) {
                task.status = TaskStatus::Pending;
                task.units = Units::ONE;
            }
        }

        let before: HashMap<&str, &Task> = base.tasks.iter().map(|t| (t.id.as_str(), t)).collect();
        let groups = next.group_ids();
        let patched: HashMap<&str, &TaskPatch> =
            update.patches.iter().map(|p| (p.id.as_str(), p)).collect();
        let mut changes = Vec::new();
        for task in &mut next.tasks {
            let old = before.get(task.id.as_str()).copied();
            if old.is_some_and(|old| same_state(old, task)) {
                continue;
            }
            let is_group = groups.contains(&task.id);
            let note = match old {
                None => "added".to_string(),
                Some(_) => patched
                    .get(task.id.as_str())
                    .and_then(|patch| patch.note.clone())
                    .unwrap_or_else(|| "dropped with its parent".to_string()),
            };
            changes.push((
                update.patches.iter().position(|p| p.id == task.id),
                describe_change(old, task, is_group),
            ));
            task.history.push_back(Revision {
                revision,
                status: task.status,
                units: (!is_group).then_some(task.units),
                note,
            });
            while task.history.len() > HISTORY_PER_TASK {
                task.history.pop_front();
            }
        }
        if changes.is_empty() && !started {
            return Ok(Applied {
                plan: base,
                changes: Vec::new(),
            });
        }
        // Patched tasks in patch order, then the ones a drop carried along.
        changes.sort_by_key(|(position, _)| position.unwrap_or(usize::MAX));
        next.revision = revision;
        Ok(Applied {
            plan: next,
            changes: changes.into_iter().map(|(_, change)| change).collect(),
        })
    }

    /// Merges one patch into the plan, before the whole-tree checks.
    fn merge(&mut self, patch: &TaskPatch) -> Result<(), PlanError> {
        let id = &patch.id;
        let Some(index) = self.tasks.iter().position(|task| &task.id == id) else {
            let Some(title) = patch.title.clone() else {
                return Err(PlanError::UnknownTask { id: id.clone() });
            };
            let units = patch.done.unwrap_or(Units::ONE);
            let status = leaf_status(patch.status, patch.done, None, units)?;
            if matches!(status, TaskStatus::Blocked | TaskStatus::Dropped) && patch.note.is_none() {
                return Err(PlanError::NoteRequired { id: id.clone() });
            }
            self.tasks.push(Task {
                id: id.clone(),
                parent: patch.parent.clone().flatten(),
                title,
                weight: patch.weight.unwrap_or(1),
                note: patch.note.clone().unwrap_or_default(),
                status,
                units: finish_units(status, units, id)?,
                history: VecDeque::new(),
            });
            return Ok(());
        };
        let task = &mut self.tasks[index];
        if task.status == TaskStatus::Dropped && patch.status.is_none() && patch.done.is_some() {
            return Err(PlanError::RestoreNeedsStatus { id: id.clone() });
        }
        let units = patch.done.unwrap_or(task.units);
        let status = leaf_status(patch.status, patch.done, Some(task.status), units)?;
        let units = finish_units(status, units, id)?;
        let parent = patch.parent.clone().unwrap_or_else(|| task.parent.clone());
        let weight = patch.weight.unwrap_or(task.weight);
        let changes_state = status != task.status
            || units != task.units
            || parent != task.parent
            || weight != task.weight;
        if changes_state && patch.note.is_none() {
            return Err(PlanError::NoteRequired { id: id.clone() });
        }
        task.status = status;
        task.units = units;
        task.parent = parent;
        task.weight = weight;
        if let Some(title) = &patch.title {
            task.title = title.clone();
        }
        if let Some(note) = &patch.note {
            task.note = note.clone();
        }
        Ok(())
    }

    /// Every parent exists, no cycles, depth and size limits.
    fn check_tree(&self) -> Result<(), PlanError> {
        if self.tasks.len() > MAX_TASKS {
            return Err(PlanError::TooManyTasks { max: MAX_TASKS });
        }
        let parents: HashMap<&str, Option<&str>> = self
            .tasks
            .iter()
            .map(|task| (task.id.as_str(), task.parent.as_deref()))
            .collect();
        for task in &self.tasks {
            let mut depth = 0;
            let mut parent = task.parent.as_deref();
            while let Some(id) = parent {
                if id == task.id {
                    return Err(PlanError::Cycle {
                        id: task.id.clone(),
                    });
                }
                let Some(&grandparent) = parents.get(id) else {
                    return Err(PlanError::UnknownParent {
                        id: task.id.clone(),
                        parent: id.to_string(),
                    });
                };
                depth += 1;
                if depth > MAX_DEPTH {
                    // A cycle that does not pass through `task` also ends
                    // here; it is reported from one of its own members.
                    return Err(PlanError::TooDeep {
                        id: task.id.clone(),
                        max: MAX_DEPTH,
                    });
                }
                parent = grandparent;
            }
        }
        Ok(())
    }

    /// Drops everything under a task this update dropped.
    fn drop_subtrees(&mut self, update: &PlanUpdate) {
        let mut dropped: HashSet<String> = update
            .patches
            .iter()
            .filter(|patch| patch.status == Some(TaskStatus::Dropped))
            .map(|patch| patch.id.clone())
            .collect();
        loop {
            let mut grew = false;
            for task in &mut self.tasks {
                if task.status != TaskStatus::Dropped
                    && task.parent.as_ref().is_some_and(|p| dropped.contains(p))
                {
                    task.status = TaskStatus::Dropped;
                    dropped.insert(task.id.clone());
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }
    }

    fn check_dropped_parents(&self) -> Result<(), PlanError> {
        for task in &self.tasks {
            if task.status == TaskStatus::Dropped {
                continue;
            }
            if let Some(parent) = &task.parent
                && self
                    .task(parent)
                    .is_some_and(|p| p.status == TaskStatus::Dropped)
            {
                return Err(PlanError::DroppedParent {
                    id: task.id.clone(),
                    parent: parent.clone(),
                });
            }
        }
        Ok(())
    }

    fn task(&self, id: &str) -> Option<&Task> {
        self.tasks.iter().find(|task| task.id == id)
    }

    /// Live (not dropped) children of `id`, in order.
    fn children_of(&self, id: &str) -> Vec<String> {
        self.tasks
            .iter()
            .filter(|task| task.status != TaskStatus::Dropped && task.parent.as_deref() == Some(id))
            .map(|task| task.id.clone())
            .collect()
    }

    /// Tasks with at least one live child.
    fn group_ids(&self) -> HashSet<String> {
        self.tasks
            .iter()
            .filter(|task| task.status != TaskStatus::Dropped)
            .filter_map(|task| task.parent.clone())
            .collect()
    }

    /// Whether any leaf is pending, active or blocked.
    pub(crate) fn has_open_work(&self) -> bool {
        let groups = self.group_ids();
        self.tasks
            .iter()
            .any(|task| !groups.contains(&task.id) && task.status.is_open())
    }

    /// The view: tree order, derived status and progress for every row.
    pub(crate) fn view(&self) -> PlanView {
        let (measures, root) = self.measure();
        let groups = self.group_ids();
        let mut rows = Vec::with_capacity(self.tasks.len());
        let mut counts = LeafCounts::default();
        for (index, depth) in self.tree_order() {
            let task = &self.tasks[index];
            let is_group = groups.contains(&task.id);
            let measure = measures[index];
            if !is_group {
                match task.status {
                    TaskStatus::Done => counts.done += 1,
                    TaskStatus::Active => counts.active += 1,
                    TaskStatus::Pending => counts.pending += 1,
                    TaskStatus::Blocked => counts.blocked += 1,
                    TaskStatus::Dropped => {}
                }
            }
            rows.push(PlanRow {
                id: task.id.clone(),
                depth: depth as u8,
                title: task.title.clone(),
                status: measure.status,
                is_group,
                done: (!is_group).then(|| task.units.pair()),
                progress: measure.parts.done,
                weight: task.weight,
                note: task.note.clone(),
                history: task
                    .history
                    .iter()
                    .map(|revision| RevisionView {
                        revision: revision.revision,
                        status: revision.status,
                        done: revision.units.map(Units::pair),
                        note: revision.note.clone(),
                    })
                    .collect(),
            });
        }
        PlanView {
            goal: self.goal.clone(),
            revision: self.revision,
            progress: root.parts,
            counts,
            rows,
        }
    }

    /// Indices with their depth: each task after its parent, siblings in
    /// insertion order.
    fn tree_order(&self) -> Vec<(usize, usize)> {
        fn visit(plan: &Plan, parent: Option<&str>, depth: usize, out: &mut Vec<(usize, usize)>) {
            for (index, task) in plan.tasks.iter().enumerate() {
                if task.parent.as_deref() == parent {
                    out.push((index, depth));
                    visit(plan, Some(&task.id), depth + 1, out);
                }
            }
        }
        let mut out = Vec::with_capacity(self.tasks.len());
        visit(self, None, 0, &mut out);
        out
    }

    /// Measures every task, by index, plus the root.
    fn measure(&self) -> (Vec<Measure>, Measure) {
        fn of(plan: &Plan, index: usize, cache: &mut Vec<Option<Measure>>) -> Measure {
            if let Some(measure) = cache[index] {
                return measure;
            }
            let task = &plan.tasks[index];
            let measure = if task.status == TaskStatus::Dropped {
                Measure::DROPPED
            } else {
                let children = plan.live_children(Some(&task.id));
                if children.is_empty() {
                    Measure::leaf(task.status, task.units)
                } else {
                    let parts = children
                        .iter()
                        .map(|&child| (plan.tasks[child].weight, of(plan, child, cache)))
                        .collect::<Vec<_>>();
                    Measure::group(&parts)
                }
            };
            cache[index] = Some(measure);
            measure
        }
        let mut cache = vec![None; self.tasks.len()];
        let measures = (0..self.tasks.len())
            .map(|index| of(self, index, &mut cache))
            .collect();
        let roots = self
            .live_children(None)
            .into_iter()
            .map(|index| (self.tasks[index].weight, of(self, index, &mut cache)))
            .collect::<Vec<_>>();
        let root = if roots.is_empty() {
            Measure::leaf(TaskStatus::Pending, Units::ONE)
        } else {
            Measure::group(&roots)
        };
        (measures, root)
    }

    fn live_children(&self, parent: Option<&str>) -> Vec<usize> {
        self.tasks
            .iter()
            .enumerate()
            .filter(|(_, task)| {
                task.status != TaskStatus::Dropped && task.parent.as_deref() == parent
            })
            .map(|(index, _)| index)
            .collect()
    }
}

/// A leaf's status after a patch: the status given, else the one `done`
/// implies, else the one it had.
fn leaf_status(
    status: Option<TaskStatus>,
    done: Option<Units>,
    before: Option<TaskStatus>,
    units: Units,
) -> Result<TaskStatus, PlanError> {
    Ok(match (status, done) {
        (Some(status), _) => status,
        (None, Some(_)) if units.completed == units.total => TaskStatus::Done,
        (None, Some(_)) if units.completed > 0 => TaskStatus::Active,
        (None, Some(_)) => TaskStatus::Pending,
        (None, None) => before.unwrap_or(TaskStatus::Pending),
    })
}

/// Done completes every unit; pending has none completed.
fn finish_units(status: TaskStatus, units: Units, id: &str) -> Result<Units, PlanError> {
    match status {
        TaskStatus::Done => Ok(Units {
            completed: units.total,
            total: units.total,
        }),
        TaskStatus::Pending if units.completed > 0 => {
            Err(PlanError::PendingWithProgress { id: id.to_string() })
        }
        _ => Ok(units),
    }
}

fn same_state(a: &Task, b: &Task) -> bool {
    a.parent == b.parent
        && a.title == b.title
        && a.weight == b.weight
        && a.note == b.note
        && a.status == b.status
        && a.units == b.units
}

/// `fold 1/4 (was 0/4)`, `types done`, `tests added`.
fn describe_change(old: Option<&Task>, new: &Task, is_group: bool) -> String {
    let state = |task: &Task| {
        if is_group || task.units.total == 1 || task.status == TaskStatus::Dropped {
            task.status.label().to_string()
        } else if task.status == TaskStatus::Done {
            "done".to_string()
        } else {
            format!(
                "{} {}/{}",
                task.status.label(),
                task.units.completed,
                task.units.total
            )
        }
    };
    match old {
        None => format!("{} added", new.id),
        Some(old) if old.status != new.status || old.units != new.units => {
            if is_group {
                format!("{} {}", new.id, new.status.label())
            } else {
                format!("{} {} (was {})", new.id, state(new), state(old))
            }
        }
        Some(old) if old.parent != new.parent => match &new.parent {
            Some(parent) => format!("{} moved under {parent}", new.id),
            None => format!("{} moved to the top level", new.id),
        },
        Some(old) if old.weight != new.weight => {
            format!("{} weight {} (was {})", new.id, new.weight, old.weight)
        }
        Some(_) => format!("{} edited", new.id),
    }
}

/// Derived status and bar shares of one task or of the root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Measure {
    status: TaskStatus,
    parts: Progress,
}

impl Measure {
    const DROPPED: Self = Self {
        status: TaskStatus::Dropped,
        parts: Progress {
            done: 0,
            active: 0,
            pending: 0,
            blocked: 0,
        },
    };

    fn leaf(status: TaskStatus, units: Units) -> Self {
        // Only a done task reads 100%; an active one with every unit counted
        // still waits for its status.
        let done = if status == TaskStatus::Done {
            FULL
        } else {
            ((u32::from(units.completed) * u32::from(FULL) / u32::from(units.total)) as u16)
                .min(FULL - 1)
        };
        let rest = FULL - done;
        let mut parts = Progress {
            done,
            ..Progress::default()
        };
        match status {
            TaskStatus::Active => parts.active = rest,
            TaskStatus::Blocked => parts.blocked = rest,
            _ => parts.pending = rest,
        }
        Self { status, parts }
    }

    /// Children's shares weighted by their weights. `done` rounds down, so a
    /// group reads 100% only when every leaf under it is done; the other
    /// shares take the rest by largest remainder, so the four sum to 1000.
    fn group(children: &[(u8, Measure)]) -> Self {
        let total_weight: u32 = children.iter().map(|(w, _)| u32::from(*w)).sum();
        let sum = |pick: fn(&Progress) -> u16| -> u32 {
            children
                .iter()
                .map(|(w, m)| u32::from(*w) * u32::from(pick(&m.parts)))
                .sum()
        };
        let done = (sum(|p| p.done) / total_weight) as u16;
        let raw = [sum(|p| p.active), sum(|p| p.pending), sum(|p| p.blocked)];
        let mut shares = raw.map(|value| value / total_weight);
        let mut left = u32::from(FULL - done) - shares.iter().sum::<u32>();
        let mut order = [0, 1, 2];
        order.sort_by_key(|&i| std::cmp::Reverse(raw[i] % total_weight));
        for &i in order.iter().cycle() {
            if left == 0 {
                break;
            }
            if raw[i] > 0 {
                shares[i] += 1;
                left -= 1;
            } else if raw.iter().all(|&r| r == 0) {
                shares[1] += left;
                break;
            }
        }
        let statuses = children.iter().map(|(_, m)| m.status);
        let status = if children.iter().all(|(_, m)| m.status == TaskStatus::Done) {
            TaskStatus::Done
        } else if statuses.clone().any(|s| s == TaskStatus::Active) || done > 0 {
            TaskStatus::Active
        } else if statuses.clone().any(|s| s == TaskStatus::Blocked) {
            TaskStatus::Blocked
        } else {
            TaskStatus::Pending
        };
        Self {
            status,
            parts: Progress {
                done,
                active: shares[0] as u16,
                pending: shares[1] as u16,
                blocked: shares[2] as u16,
            },
        }
    }
}

/// Why an update was refused. Each message names the task, says how to fix
/// the call, and ends with "Nothing was changed." because the model must
/// know the batch is all or nothing.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum PlanError {
    #[error("There is no plan yet. Start one with goal and its first tasks. Nothing was changed.")]
    NoPlan,
    #[error("Task {id} does not exist. Give it a title to add it. Nothing was changed.")]
    UnknownTask { id: String },
    #[error(
        "Task {id} names parent {parent}, which does not exist. Add the parent in the same call, or leave parent out. Nothing was changed."
    )]
    UnknownParent { id: String, parent: String },
    #[error("Task {id} would be inside itself. Nothing was changed.")]
    Cycle { id: String },
    #[error("Task {id} is nested more than {max} levels deep. Nothing was changed.")]
    TooDeep { id: String, max: usize },
    #[error("Task {id} appears twice in this call. Nothing was changed.")]
    DuplicateId { id: String },
    #[error(
        "{id} has subtasks ({children}) and takes its status and progress from them. Set status on those subtasks, or drop {id} with status \"dropped\". Nothing was changed."
    )]
    GroupState { id: String, children: String },
    #[error(
        "Task {id} changes its status, progress, parent or weight, or is blocked or dropped; say why in note. Nothing was changed."
    )]
    NoteRequired { id: String },
    #[error(
        "Task {id} is under {parent}, which is dropped. Move it or drop it in the same call. Nothing was changed."
    )]
    DroppedParent { id: String, parent: String },
    #[error(
        "Task {id} is pending but has completed units; make it active or set done to [0, total]. Nothing was changed."
    )]
    PendingWithProgress { id: String },
    #[error("Task {id} is dropped; give a status to bring it back. Nothing was changed.")]
    RestoreNeedsStatus { id: String },
    #[error(
        "A plan holds at most {max} tasks. Group or drop tasks you no longer need. Nothing was changed."
    )]
    TooManyTasks { max: usize },
    #[error("tasks[{index}].{field}: {reason}. Nothing was changed.")]
    Field {
        index: usize,
        field: &'static str,
        reason: String,
    },
    #[error("{0}. Nothing was changed.")]
    Input(String),
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use proptest::prelude::*;
    use serde_json::json;

    pub(crate) fn apply(current: Option<&Plan>, input: Value) -> Result<Applied, PlanError> {
        Plan::apply(current, &PlanUpdate::parse(&input)?)
    }

    fn start() -> Plan {
        apply(
            None,
            json!({"goal": "Add a Timeline", "tasks": [
                {"id": "core", "title": "Plan model in core", "weight": 3},
                {"id": "types", "parent": "core", "title": "Types", "status": "active"},
                {"id": "fold", "parent": "core", "title": "Fold"},
                {"id": "tui", "title": "Timeline view"}
            ]}),
        )
        .unwrap()
        .plan
    }

    fn row<'a>(view: &'a PlanView, id: &str) -> &'a PlanRow {
        view.rows.iter().find(|row| row.id == id).unwrap()
    }

    #[test]
    fn a_new_plan_lists_tasks_in_tree_order_with_groups_derived() {
        let view = start().view();
        let order = view
            .rows
            .iter()
            .map(|r| (r.id.as_str(), r.depth))
            .collect::<Vec<_>>();
        assert_eq!(order, [("core", 0), ("types", 1), ("fold", 1), ("tui", 0)]);
        assert_eq!(view.revision, 1);
        assert!(row(&view, "core").is_group);
        assert_eq!(row(&view, "core").status, TaskStatus::Active);
        assert_eq!(row(&view, "core").done, None);
        assert_eq!(
            view.counts,
            LeafCounts {
                done: 0,
                active: 1,
                pending: 2,
                blocked: 0
            }
        );
        assert_eq!(row(&view, "types").history[0].note, "added");
    }

    #[test]
    fn weights_split_progress_among_siblings_and_survive_a_split() {
        // dsh's example: a done task of weight 3 beside an unstarted task of
        // weight 1 is 75% of the work.
        let plan = apply(
            None,
            json!({"goal": "g", "tasks": [
                {"id": "a", "title": "A", "weight": 3, "status": "done"},
                {"id": "b", "title": "B"}
            ]}),
        )
        .unwrap()
        .plan;
        assert_eq!(plan.view().progress.done, 750);

        // Splitting `b` into children keeps its quarter.
        let plan = apply(
            Some(&plan),
            json!({"tasks": [
                {"id": "b1", "parent": "b", "title": "B1", "status": "done"},
                {"id": "b2", "parent": "b", "title": "B2"}
            ]}),
        )
        .unwrap()
        .plan;
        let view = plan.view();
        assert_eq!(view.progress.done, 875);
        assert_eq!(row(&view, "b").progress, 500);
    }

    #[test]
    fn progress_shares_always_sum_to_1000_and_done_reaches_1000_only_when_finished() {
        let plan = apply(
            None,
            json!({"goal": "g", "tasks": [
                {"id": "a", "title": "A", "weight": 7, "done": [1, 3]},
                {"id": "b", "title": "B", "weight": 2, "status": "blocked", "note": "waits"},
                {"id": "c", "title": "C", "weight": 5, "status": "done"}
            ]}),
        )
        .unwrap()
        .plan;
        let p = plan.view().progress;
        assert_eq!(p.done + p.active + p.pending + p.blocked, 1000);
        assert!(p.done < 1000);
        assert!(p.blocked > 0 && p.active > 0);
    }

    #[test]
    fn done_units_set_the_status_when_none_is_given() {
        let plan = start();
        let plan = apply(
            Some(&plan),
            json!({"tasks": [{"id": "fold", "done": [1, 4], "note": "half"}]}),
        )
        .unwrap()
        .plan;
        assert_eq!(row(&plan.view(), "fold").status, TaskStatus::Active);
        assert_eq!(row(&plan.view(), "fold").done, Some([1, 4]));
        let plan = apply(
            Some(&plan),
            json!({"tasks": [{"id": "fold", "done": [4, 4], "note": "all"}]}),
        )
        .unwrap()
        .plan;
        assert_eq!(row(&plan.view(), "fold").status, TaskStatus::Done);
    }

    #[test]
    fn a_refused_batch_changes_nothing() {
        let plan = start();
        let err = apply(
            Some(&plan),
            json!({"tasks": [
                {"id": "types", "status": "done", "note": "ok"},
                {"id": "ghost", "status": "done", "note": "?"}
            ]}),
        )
        .err()
        .unwrap();
        assert_eq!(err, PlanError::UnknownTask { id: "ghost".into() });
        assert_eq!(plan, start());
    }

    #[test]
    fn an_update_applied_twice_reports_no_change_the_second_time() {
        let plan = start();
        let update = json!({"tasks": [{"id": "types", "status": "done", "note": "ok"}]});
        let once = apply(Some(&plan), update.clone()).unwrap();
        assert_eq!(once.changes, ["types done (was active)"]);
        let twice = apply(Some(&once.plan), update).unwrap();
        assert!(twice.changes.is_empty());
        assert_eq!(twice.plan, once.plan);
    }

    #[test]
    fn the_current_goal_again_keeps_the_plan_and_a_new_goal_replaces_it() {
        let plan = start();
        let same = apply(
            Some(&plan),
            json!({"goal": "Add a Timeline", "tasks": [{"id": "tui", "title": "Timeline view"}]}),
        )
        .unwrap();
        assert!(same.changes.is_empty());
        assert_eq!(same.plan, plan);

        let new = apply(
            Some(&plan),
            json!({"goal": "Fix the flaky test", "tasks": [{"id": "repro", "title": "Reproduce"}]}),
        )
        .unwrap();
        assert_eq!(new.plan.view().goal, "Fix the flaky test");
        assert_eq!(new.plan.view().rows.len(), 1);
        assert_eq!(new.plan.view().revision, 1);
    }

    #[test]
    fn changing_state_parent_or_weight_needs_a_note() {
        let plan = start();
        for patch in [
            json!({"id": "types", "status": "done"}),
            json!({"id": "fold", "done": [1, 2]}),
            json!({"id": "tui", "parent": "core"}),
            json!({"id": "tui", "weight": 2}),
        ] {
            assert_eq!(
                apply(Some(&plan), json!({"tasks": [patch]})).err(),
                Some(PlanError::NoteRequired {
                    id: patch["id"].as_str().unwrap().into()
                })
            );
        }
        // A title alone needs none, and a new blocked task does.
        assert!(
            apply(
                Some(&plan),
                json!({"tasks": [{"id": "tui", "title": "TUI"}]})
            )
            .is_ok()
        );
        assert_eq!(
            apply(
                Some(&plan),
                json!({"tasks": [{"id": "x", "title": "X", "status": "blocked"}]})
            )
            .err(),
            Some(PlanError::NoteRequired { id: "x".into() })
        );
    }

    #[test]
    fn a_group_takes_its_state_from_its_children() {
        let plan = start();
        let err = apply(
            Some(&plan),
            json!({"tasks": [{"id": "core", "status": "done", "note": "all"}]}),
        )
        .err()
        .unwrap();
        assert_eq!(
            err,
            PlanError::GroupState {
                id: "core".into(),
                children: "types, fold".into()
            }
        );
        assert!(err.to_string().ends_with("Nothing was changed."));
    }

    #[test]
    fn dropping_a_group_drops_its_subtree_and_restoring_needs_a_status() {
        let plan = start();
        let dropped = apply(
            Some(&plan),
            json!({"tasks": [{"id": "core", "status": "dropped", "note": "out of scope"}]}),
        )
        .unwrap();
        let view = dropped.plan.view();
        assert!(
            ["core", "types", "fold"]
                .iter()
                .all(|id| row(&view, id).status == TaskStatus::Dropped)
        );
        assert_eq!(dropped.changes[0], "core dropped (was pending)");
        assert_eq!(view.counts.total(), 1);
        assert_eq!(
            row(&view, "fold").history.last().unwrap().note,
            "dropped with its parent"
        );

        assert_eq!(
            apply(
                Some(&dropped.plan),
                json!({"tasks": [{"id": "fold", "done": [1, 2], "note": "back"}]})
            )
            .err(),
            Some(PlanError::RestoreNeedsStatus { id: "fold".into() })
        );
        assert_eq!(
            apply(
                Some(&dropped.plan),
                json!({"tasks": [{"id": "fold", "status": "active", "note": "back"}]})
            )
            .err(),
            Some(PlanError::DroppedParent {
                id: "fold".into(),
                parent: "core".into()
            })
        );
        assert!(
            apply(
                Some(&dropped.plan),
                json!({"tasks": [{"id": "fold", "parent": null, "status": "active", "note": "back"}]})
            )
            .is_ok()
        );
    }

    #[test]
    fn the_tree_refuses_unknown_parents_cycles_and_deep_nesting() {
        let plan = start();
        assert_eq!(
            apply(
                Some(&plan),
                json!({"tasks": [{"id": "x", "parent": "nope", "title": "X"}]})
            )
            .err(),
            Some(PlanError::UnknownParent {
                id: "x".into(),
                parent: "nope".into()
            })
        );
        assert!(matches!(
            apply(
                Some(&plan),
                json!({"tasks": [{"id": "core", "parent": "types", "note": "loop"}]})
            )
            .err(),
            Some(PlanError::Cycle { .. } | PlanError::TooDeep { .. })
        ));
        let deep = apply(
            Some(&plan),
            json!({"tasks": [
                {"id": "d1", "parent": "types", "title": "1"},
                {"id": "d2", "parent": "d1", "title": "2"},
                {"id": "d3", "parent": "d2", "title": "3"}
            ]}),
        );
        assert_eq!(
            deep.err(),
            Some(PlanError::TooDeep {
                id: "d3".into(),
                max: MAX_DEPTH
            })
        );
    }

    #[test]
    fn the_boundary_refuses_bad_fields() {
        for (input, expected) in [
            (json!({"goal": "g", "tasks": []}), "tasks must list"),
            (
                json!({"goal": "g", "tasks": [{"id": "a b", "title": "A"}]}),
                "tasks[0].id",
            ),
            (
                json!({"goal": "g", "tasks": [{"id": "a", "title": "A", "done": [3, 2]}]}),
                "tasks[0].done",
            ),
            (
                json!({"goal": "g", "tasks": [{"id": "a", "title": "A", "weight": 0}]}),
                "tasks[0].weight",
            ),
            (
                json!({"goal": "g", "tasks": [{"id": "a", "title": "A", "extra": 1}]}),
                "unknown field",
            ),
            (
                json!({"goal": "g", "tasks": [{"id": "a", "title": "A"}, {"id": "a", "title": "B"}]}),
                "appears twice",
            ),
            (
                json!({"goal": "g", "tasks": [{"id": "a", "title": "A", "status": "paused"}]}),
                "unknown variant",
            ),
            (json!({"tasks": [{"id": "a", "title": "A"}]}), "no plan yet"),
        ] {
            let err = apply(None, input).err().unwrap().to_string();
            assert!(err.contains(expected), "{err}");
            assert!(err.ends_with("Nothing was changed."), "{err}");
        }
    }

    #[test]
    fn history_keeps_the_latest_changes_per_task() {
        let mut plan = start();
        for step in 1..=5 {
            plan = apply(Some(&plan), json!({"tasks": [{"id": "fold", "done": [step, 6], "note": format!("step {step}")}]}))
                .unwrap()
                .plan;
        }
        let view = plan.view();
        let history = &row(&view, "fold").history;
        assert_eq!(history.len(), HISTORY_PER_TASK);
        assert_eq!(history.last().unwrap().note, "step 5");
        assert_eq!(history.last().unwrap().revision, 6);
    }

    proptest! {
        #[test]
        fn shares_sum_to_1000_for_any_weights_and_states(
            leaves in prop::collection::vec((1u8..=100, 0u16..=5, 1u16..=5, 0usize..3), 1..12)
        ) {
            let tasks = leaves
                .iter()
                .enumerate()
                .map(|(i, &(weight, completed, total, state))| {
                    let completed = completed.min(total);
                    let status = match state {
                        0 => "active",
                        1 => "blocked",
                        _ => if completed == total { "done" } else { "active" },
                    };
                    json!({"id": format!("t{i}"), "parent": if i > 0 && i % 3 == 0 { json!("t0") } else { Value::Null },
                           "title": "T", "weight": weight, "done": [completed, total], "status": status, "note": "n"})
                })
                .collect::<Vec<_>>();
            let applied = apply(None, json!({"goal": "g", "tasks": tasks}));
            if let Ok(applied) = applied {
                let view = applied.plan.view();
                let p = view.progress;
                prop_assert_eq!(p.done + p.active + p.pending + p.blocked, 1000);
                prop_assert_eq!(p.done == 1000, view.counts.done == view.counts.total());
                for row in &view.rows {
                    prop_assert!(row.progress <= 1000);
                }
            }
        }
    }
}
