//! Renders the Timeline view: a segmented bar for the whole plan, then one
//! line per task in tree order, scrolled so the selected task, or the one
//! the agent is on, stays in view.

use crate::timeline::TimelineModel;
use crate::view::cells::{cells, truncate_cells};
use crate::view::theme::*;
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use socium_core::plan::{PlanRow, Progress, TaskStatus};

const GUTTER: usize = 2;
const INDENT: usize = 2;
const BAR_MIN: usize = 10;
const BAR_MAX: usize = 48;
/// Rows show their own percentage from this width.
const PERCENT_MIN_WIDTH: usize = 40;
const OK: Color = Color::Green;

/// Visible lines for a pane of `width` × `height` cells. `focused` marks
/// the selection.
pub(crate) fn timeline_lines(
    model: &TimelineModel,
    focused: bool,
    width: usize,
    height: usize,
) -> Vec<Line<'static>> {
    if height == 0 {
        return Vec::new();
    }
    let Some(plan) = model.plan() else {
        return vec![Line::from(Span::styled(
            "  No plan yet. For work with several steps the agent lists its tasks here.",
            Style::default().fg(MUTED),
        ))];
    };
    // One cell stays free before the right border.
    let inner = width.saturating_sub(GUTTER + 1);
    let mut header = vec![Span::raw(" ".repeat(GUTTER))];
    header.extend(progress_bar(
        &plan.progress,
        (inner / 4).clamp(BAR_MIN, BAR_MAX),
    ));
    let counts = plan.counts;
    let mut facts = format!(
        " {}% · {} of {} done",
        plan.progress.done / 10,
        counts.done,
        counts.total()
    );
    for (count, label) in [(counts.active, "active"), (counts.blocked, "blocked")] {
        if count > 0 {
            facts.push_str(&format!(" · {count} {label}"));
        }
    }
    header.push(Span::styled(
        truncate_cells(
            &facts,
            inner.saturating_sub((inner / 4).clamp(BAR_MIN, BAR_MAX)),
        ),
        Style::default().fg(MUTED),
    ));

    let mut lines = Vec::new();
    let mut current = None;
    for row in model.visible_rows() {
        let selected = model.is_selected(&row.id);
        if selected {
            current = Some(lines.len());
        }
        lines.push(row_line(row, inner, focused && selected));
        if model.is_expanded(&row.id) {
            lines.extend(history_lines(row, inner));
        } else if !row.note.is_empty() && matches!(row.status, TaskStatus::Blocked) {
            lines.push(note_line(row, &format!("blocked: {}", row.note), inner));
        }
    }

    let room = height - 1;
    let max_start = lines.len().saturating_sub(room);
    let start = current
        .map(|current| current.saturating_sub(room / 3).min(max_start))
        .unwrap_or(0);
    let mut out = vec![Line::from(header)];
    out.extend(lines.into_iter().skip(start).take(room));
    out
}

/// `██████▓▓░░░░░░▒▒` in done, active, pending and blocked shares, exactly
/// `cells` wide. Every share above zero gets a cell, so a blocked task
/// never disappears from the bar; the rest go by largest remainder.
fn progress_bar(progress: &Progress, cells: usize) -> Vec<Span<'static>> {
    let shares = [
        (progress.done, "█", OK),
        (progress.active, "▓", ACCENT),
        (progress.pending, "░", MUTED),
        (progress.blocked, "▒", ERROR),
    ];
    let raw = shares.map(|(share, ..)| usize::from(share));
    let total = raw.iter().sum::<usize>().max(1);
    let mut widths = raw.map(|share| {
        if share > 0 {
            (share * cells / total).max(1)
        } else {
            0
        }
    });
    while widths.iter().sum::<usize>() > cells {
        let widest = (0..4).max_by_key(|&i| widths[i]).expect("four shares");
        widths[widest] -= 1;
    }
    let mut order = [0, 1, 2, 3];
    order.sort_by_key(|&i| std::cmp::Reverse(raw[i] * cells % total));
    let mut next = order.iter().filter(|&&i| raw[i] > 0).cycle();
    while widths.iter().sum::<usize>() < cells {
        match next.next() {
            Some(&i) => widths[i] += 1,
            None => widths[2] = cells,
        }
    }
    shares
        .iter()
        .zip(widths)
        .filter(|(_, width)| *width > 0)
        .map(|((_, glyph, color), width)| {
            Span::styled(glyph.repeat(width), Style::default().fg(*color))
        })
        .collect()
}

/// `› ● fold  Rebuild the plan from history   1/4  25%`
fn row_line(row: &PlanRow, width: usize, selected: bool) -> Line<'static> {
    let (glyph, color) = glyph(row.status);
    let marker = if selected { "› " } else { "  " };
    let indent = " ".repeat(INDENT * usize::from(row.depth));
    let mut tail = String::new();
    if let Some([completed, total]) = row.done
        && total > 1
        && row.status != TaskStatus::Done
    {
        tail.push_str(&format!("  {completed}/{total}"));
    }
    if width >= PERCENT_MIN_WIDTH {
        tail.push_str(&format!("  {:>3}%", row.progress / 10));
    }
    let head = format!("{indent}{glyph} ");
    let id = format!("{} ", row.id);
    let room = width
        .saturating_sub(cells(&head) + cells(&id) + cells(&tail))
        .max(1);
    let title = truncate_cells(&row.title, room);
    let pad = room.saturating_sub(cells(&title));
    let muted = row.status == TaskStatus::Done;
    Line::from(vec![
        Span::styled(marker, Style::default().fg(ACCENT)),
        Span::raw(indent),
        Span::styled(format!("{glyph} "), Style::default().fg(color)),
        Span::styled(id, Style::default().fg(MUTED)),
        Span::styled(title, Style::default().fg(if muted { MUTED } else { TEXT })),
        Span::raw(" ".repeat(pad)),
        Span::styled(tail, Style::default().fg(MUTED)),
    ])
}

/// `      r5 ● 1/4  replay skips errored calls`, oldest first.
fn history_lines(row: &PlanRow, width: usize) -> Vec<Line<'static>> {
    row.history
        .iter()
        .map(|revision| {
            let (glyph, color) = glyph(revision.status);
            let units = match revision.done {
                Some([completed, total]) if total > 1 => format!(" {completed}/{total}"),
                _ => String::new(),
            };
            let indent = " ".repeat(GUTTER + INDENT * (usize::from(row.depth) + 1));
            let head = format!("r{} ", revision.revision);
            let room = width.saturating_sub(cells(&indent) + cells(&head) + 2 + cells(&units) + 2);
            Line::from(vec![
                Span::raw(indent),
                Span::styled(head, Style::default().fg(MUTED)),
                Span::styled(glyph, Style::default().fg(color)),
                Span::styled(units, Style::default().fg(MUTED)),
                Span::raw("  "),
                Span::styled(
                    truncate_cells(&revision.note, room),
                    Style::default().fg(TEXT),
                ),
            ])
        })
        .collect()
}

fn note_line(row: &PlanRow, text: &str, width: usize) -> Line<'static> {
    let indent = " ".repeat(GUTTER + INDENT * (usize::from(row.depth) + 1));
    let room = width.saturating_sub(cells(&indent) + 2);
    Line::from(vec![
        Span::raw(indent),
        Span::styled("└ ", Style::default().fg(MUTED)),
        Span::styled(truncate_cells(text, room), Style::default().fg(MUTED)),
    ])
}

fn glyph(status: TaskStatus) -> (&'static str, Color) {
    match status {
        TaskStatus::Done => ("✓", OK),
        TaskStatus::Active => ("●", ACCENT),
        TaskStatus::Pending => ("○", MUTED),
        TaskStatus::Blocked => ("⊘", ERROR),
        TaskStatus::Dropped => ("–", MUTED),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::tests::sample_plan;

    fn text(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn the_bar_is_exactly_as_wide_as_asked_for_any_shares() {
        for (done, active, pending, blocked) in [
            (1000, 0, 0, 0),
            (333, 333, 334, 0),
            (1, 1, 1, 997),
            (0, 0, 1000, 0),
        ] {
            for cells in [BAR_MIN, 17, BAR_MAX] {
                let bar = progress_bar(
                    &Progress {
                        done,
                        active,
                        pending,
                        blocked,
                    },
                    cells,
                );
                let width: usize = bar.iter().map(|span| span.content.chars().count()).sum();
                assert_eq!(width, cells);
                assert_eq!(
                    bar.len(),
                    [done, active, pending, blocked]
                        .iter()
                        .filter(|s| **s > 0)
                        .count()
                );
            }
        }
    }

    #[test]
    fn a_short_pane_keeps_the_active_task_in_view() {
        let mut model = TimelineModel::default();
        model.show(Some(sample_plan()));
        let lines = text(&timeline_lines(&model, false, 78, 3));
        assert_eq!(lines.len(), 3);
        assert!(
            lines[0].contains("32% · 2 of 7 done · 1 active · 1 blocked"),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.contains("● fold")),
            "{lines:?}"
        );
        assert!(
            lines.iter().all(|line| !line.contains("Task old")),
            "dropped rows are hidden"
        );
        assert!(
            lines.iter().all(|line| !line.starts_with("›")),
            "unfocused: no marker"
        );
    }

    #[test]
    fn focus_marks_the_selection_and_enter_shows_its_history() {
        let mut model = TimelineModel::default();
        model.show(Some(sample_plan()));
        model.toggle_history();
        let lines = text(&timeline_lines(&model, true, 60, 20));
        let fold = lines.iter().position(|line| line.contains("fold")).unwrap();
        assert!(lines[fold].starts_with("› "), "{lines:?}");
        assert!(
            lines[fold + 1].contains("r1") && lines[fold + 1].contains("added"),
            "{lines:?}"
        );
        assert!(lines.iter().all(|line| cells(line) <= 60), "{lines:?}");
    }

    #[test]
    fn without_a_plan_it_says_what_will_appear() {
        let lines = text(&timeline_lines(&TimelineModel::default(), false, 40, 5));
        assert!(lines[0].contains("No plan yet"));
    }
}
