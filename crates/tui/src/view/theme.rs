use crate::view::text_layout::slice_chars;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

pub(crate) const BG: Color = Color::Rgb(10, 14, 24);
pub(crate) const TEXT: Color = Color::Rgb(220, 223, 235);
pub(crate) const MUTED: Color = Color::Rgb(118, 124, 151);
pub(crate) const BLUE: Color = Color::Rgb(102, 160, 255);
pub(crate) const LILAC: Color = Color::Rgb(187, 145, 255);
pub(crate) const PINK: Color = Color::Rgb(232, 126, 169);
pub(crate) const SOFT: Color = Color::Rgb(83, 91, 117);
pub(crate) const CODE_BORDER: Color = Color::Rgb(104, 94, 72);
pub(crate) const CODE_TEXT: Color = Color::Rgb(198, 204, 225);

pub(crate) const LOGO_WORDMARK: [&str; 6] = [
    "███████╗  ██████╗   ██████╗  ██╗  ██╗   ██╗  ███╗   ███╗",
    "██╔════╝ ██╔═══██╗ ██╔════╝  ██║  ██║   ██║  ████╗ ████║",
    "███████╗ ██║   ██║ ██║       ██║  ██║   ██║  ██╔████╔██║",
    "╚════██║ ██║   ██║ ██║       ██║  ██║   ██║  ██║╚██╔╝██║",
    "███████║ ╚██████╔╝ ╚██████╗  ██║  ╚██████╔╝  ██║ ╚═╝ ██║",
    "╚══════╝  ╚═════╝   ╚═════╝  ╚═╝   ╚═════╝   ╚═╝     ╚═╝",
];

pub(crate) fn build_logo_rows(rows: &[&str]) -> Vec<Line<'static>> {
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
