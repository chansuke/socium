//! Scenarios: key presses and typed text sent to the app, with what each
//! frame must and must not show. Each file in `crates/tui/scenarios/` is one
//! scenario; `scripts/verify tui` runs them and keeps every frame as text
//! under `target/verify/tui/`.
use crate::app::{App, SubmitAction};
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use unicode_width::UnicodeWidthStr;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scenario {
    /// What the scenario shows, in a sentence.
    description: String,
    /// Columns and rows of the terminal; 80 by 24 when left out.
    size: Option<[u16; 2]>,
    /// The state the app starts in: `empty` or `activity`.
    #[serde(default = "empty")]
    fixture: String,
    step: Vec<Step>,
}

fn empty() -> String {
    "empty".into()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Step {
    /// Text typed one character at a time.
    #[serde(rename = "type")]
    typed: Option<String>,
    /// Keys pressed in order, such as `Esc`, `Ctrl+a` or `Shift+Enter`.
    #[serde(default)]
    keys: Vec<String>,
    /// Text the frame must show after this step.
    #[serde(default)]
    expect: Vec<String>,
    /// Text the frame must not show after this step.
    #[serde(default)]
    absent: Vec<String>,
}

/// The frame as rows of text, a wide character read once.
pub(crate) fn render_at(app: &App, width: u16, height: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| app.draw(frame)).unwrap();
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            let mut row = String::new();
            let mut x = 0;
            while x < buffer.area.width {
                let symbol = buffer[(x, y)].symbol();
                row.push_str(symbol);
                x += symbol.width().max(1) as u16;
            }
            row
        })
        .collect()
}

fn fixture(name: &str) -> App {
    match name {
        "empty" => App::for_test(),
        "activity" => {
            let mut app = App::for_test();
            app.push_message(crate::transcript::Speaker::User, "explain the view");
            for event in crate::activity::tests::sample_turn() {
                app.activity.apply(event);
            }
            app
        }
        other => panic!("unknown fixture {other:?}; use \"empty\" or \"activity\""),
    }
}

fn key(name: &str) -> KeyEvent {
    let mut modifiers = KeyModifiers::NONE;
    let mut parts: Vec<&str> = name.split('+').collect();
    let code = parts.pop().unwrap_or_default();
    for part in parts {
        modifiers |= match part {
            "Ctrl" => KeyModifiers::CONTROL,
            "Shift" => KeyModifiers::SHIFT,
            "Alt" => KeyModifiers::ALT,
            other => panic!("unknown modifier {other:?} in {name:?}"),
        };
    }
    let code = match code {
        "Enter" => KeyCode::Enter,
        "Esc" => KeyCode::Esc,
        "Tab" => KeyCode::Tab,
        "Backspace" => KeyCode::Backspace,
        "Up" => KeyCode::Up,
        "Down" => KeyCode::Down,
        "Left" => KeyCode::Left,
        "Right" => KeyCode::Right,
        "PageUp" => KeyCode::PageUp,
        "PageDown" => KeyCode::PageDown,
        single if single.chars().count() == 1 => KeyCode::Char(single.chars().next().unwrap()),
        other => panic!("unknown key {other:?}"),
    };
    KeyEvent::new(code, modifiers)
}

/// Where frames are kept: `SOCIUM_VERIFY_DIR`, which `scripts/verify tui`
/// sets. Unset under a plain `cargo test`, which keeps nothing.
#[expect(
    clippy::disallowed_methods,
    reason = "the verify script names the output directory through the environment"
)]
fn frames_dir() -> Option<PathBuf> {
    std::env::var_os("SOCIUM_VERIFY_DIR").map(PathBuf::from)
}

/// Runs one scenario and returns what went wrong, if anything.
fn run(path: &Path, frames: Option<&Path>) -> Vec<String> {
    let name = path.file_stem().unwrap().to_string_lossy().into_owned();
    let text = std::fs::read_to_string(path).unwrap();
    let scenario: Scenario =
        toml::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let [width, height] = scenario.size.unwrap_or([80, 24]);
    let mut app = fixture(&scenario.fixture);
    let mut failures = Vec::new();
    let mut transcript = format!("# {name}: {}\n", scenario.description);
    for (index, step) in scenario.step.iter().enumerate() {
        let mut input = Vec::new();
        for c in step.typed.iter().flat_map(|text| text.chars()) {
            input.push(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        input.extend(step.keys.iter().map(|name| key(name)));
        let label = match (&step.typed, step.keys.is_empty()) {
            (Some(text), true) => format!("type {text:?}"),
            (Some(text), false) => format!("type {text:?}, then {}", step.keys.join(" ")),
            (None, true) => "start".to_string(),
            (None, false) => step.keys.join(" "),
        };
        let mut started = None;
        for event in input {
            match app.handle_event(Event::Key(event)) {
                Ok(SubmitAction::None) => {}
                // A scenario has no agent, so a task is not started; the
                // frame shows the app as it was when it would start.
                Ok(SubmitAction::Pending(_)) => started = Some("a request"),
                Ok(SubmitAction::Compact) => started = Some("a compaction"),
                Ok(SubmitAction::Exit) => started = Some("exiting"),
                Err(err) => failures.push(format!(
                    "{}: step {} ({label}) failed: {err}",
                    path.display(),
                    index + 1
                )),
            }
        }
        let rows = render_at(&app, width, height);
        let frame = rows.join("\n");
        transcript.push_str(&format!("\n## step {}: {label}\n", index + 1));
        if let Some(what) = started {
            transcript.push_str(&format!("(the app would start {what} here)\n"));
        }
        transcript.push_str(&frame);
        transcript.push('\n');
        for wanted in &step.expect {
            if !frame.contains(wanted.as_str()) {
                failures.push(format!(
                    "{}: step {} ({label}) does not show {wanted:?}",
                    path.display(),
                    index + 1
                ));
            }
        }
        for unwanted in &step.absent {
            if frame.contains(unwanted.as_str()) {
                failures.push(format!(
                    "{}: step {} ({label}) shows {unwanted:?}",
                    path.display(),
                    index + 1
                ));
            }
        }
        if !failures.is_empty() && frames.is_none() {
            failures.push(format!("frame after step {}:\n{frame}", index + 1));
            break;
        }
    }
    if let Some(dir) = frames {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(format!("{name}.txt")), transcript).unwrap();
    }
    failures
}

#[test]
fn every_scenario_shows_what_it_expects() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios");
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no scenarios in {}", dir.display());
    let frames = frames_dir();
    let failures: Vec<_> = paths
        .iter()
        .flat_map(|path| run(path, frames.as_deref()))
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
