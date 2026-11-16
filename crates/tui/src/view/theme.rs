use ratatui::style::Color;

// Colors follow the terminal's own theme: text and background are its
// defaults and secondary text is its dim gray, so Socium reads well on
// dark and light terminals alike. One warm accent marks Socium's own
// marks, focus, and the current choice.
pub(crate) const BG: Color = Color::Reset;
pub(crate) const TEXT: Color = Color::Reset;
pub(crate) const MUTED: Color = Color::DarkGray;
pub(crate) const BORDER: Color = Color::DarkGray;
/// Amber that keeps 3:1 contrast on a white background and 5:1 on a near-black one.
pub(crate) const ACCENT: Color = Color::Rgb(200, 130, 20);
/// Failures: a failed tool call or turn, and destructive confirmations.
pub(crate) const ERROR: Color = Color::Red;
/// The frame of the focused pane or open dialog; others use `BORDER`.
pub(crate) const FOCUS_BORDER: Color = Color::Reset;
/// Added and removed lines in an edit's diff preview.
pub(crate) const DIFF_ADDED: Color = Color::Green;
pub(crate) const DIFF_REMOVED: Color = Color::Red;
