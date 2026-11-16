//! Text for `/agents`: the saved subagents, and one subagent's conversation.

use socium_core::{SubagentSummary, TranscriptEntry, TranscriptRole};

/// Characters shown of each message; tool results can be long.
const MAX_ENTRY_CHARS: usize = 4_000;

pub(crate) fn runs_text(runs: &[SubagentSummary], now_ms: u64) -> String {
    if runs.is_empty() {
        return "No saved subagents in this session.".to_string();
    }
    let mut lines =
        vec!["Subagents of this session, newest first (/agents <id> shows one):".to_string()];
    for run in runs {
        lines.push(format!(
            "  {}  {} · {} · {} · {}",
            run.id,
            run.agent_type,
            run.status.label(),
            if run.runs == 1 {
                "1 run".to_string()
            } else {
                format!("{} runs", run.runs)
            },
            age(now_ms.saturating_sub(run.updated_at_ms))
        ));
        lines.push(format!("    {}", run.description));
    }
    lines.join("\n")
}

pub(crate) fn transcript_text(summary: &SubagentSummary, entries: &[TranscriptEntry]) -> String {
    let mut text = format!(
        "Subagent {} · {} · {} · {}\n{}",
        summary.id,
        summary.agent_type,
        summary.status.label(),
        summary.model,
        summary.description
    );
    for entry in entries {
        let speaker = match entry.role {
            TranscriptRole::User => "▸ to the subagent",
            TranscriptRole::Assistant => "▸ subagent",
            TranscriptRole::System => "▸ system",
            TranscriptRole::Tool { failed: false } => "▸ tool",
            TranscriptRole::Tool { failed: true } => "▸ tool (failed)",
        };
        text.push_str(&format!("\n\n{speaker}\n{}", bounded(&entry.text)));
    }
    text
}

fn bounded(text: &str) -> String {
    match text.char_indices().nth(MAX_ENTRY_CHARS) {
        Some((cut, _)) => format!(
            "{}\n… {} more characters",
            &text[..cut],
            text[cut..].chars().count()
        ),
        None => text.to_string(),
    }
}

fn age(elapsed_ms: u64) -> String {
    let minutes = elapsed_ms / 60_000;
    match minutes {
        0 => "just now".to_string(),
        1..60 => format!("{minutes}m ago"),
        60..1440 => format!("{}h ago", minutes / 60),
        _ => format!("{}d ago", minutes / 1440),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use socium_core::SubagentStatus;

    fn summary(id: &str, runs: usize, updated_at_ms: u64) -> SubagentSummary {
        SubagentSummary {
            id: id.into(),
            agent_type: "explorer".into(),
            description: "Find the greeting".into(),
            model: "claude-haiku-5-5".into(),
            status: SubagentStatus::Finished,
            runs,
            updated_at_ms,
        }
    }

    #[test]
    fn runs_are_listed_with_type_status_runs_and_age() {
        let now = 10 * 86_400_000;
        let text = runs_text(
            &[
                summary("toolu_b", 2, now - 5 * 60_000),
                summary("toolu_a", 1, now - 3 * 86_400_000),
            ],
            now,
        );

        assert_eq!(
            text,
            "Subagents of this session, newest first (/agents <id> shows one):\n  toolu_b  explorer · finished · 2 runs · 5m ago\n    Find the greeting\n  toolu_a  explorer · finished · 1 run · 3d ago\n    Find the greeting"
        );
        assert_eq!(runs_text(&[], now), "No saved subagents in this session.");
    }

    #[test]
    fn a_transcript_shows_who_said_what_and_cuts_long_entries() {
        let entries = [
            TranscriptEntry {
                role: TranscriptRole::User,
                text: "Where is hello?".into(),
            },
            TranscriptEntry {
                role: TranscriptRole::Tool { failed: true },
                text: "read_file hello.rs\nNo such file".into(),
            },
            TranscriptEntry {
                role: TranscriptRole::Assistant,
                text: "x".repeat(MAX_ENTRY_CHARS + 3),
            },
        ];

        let text = transcript_text(&summary("toolu_a", 1, 0), &entries);

        assert!(text.starts_with(
            "Subagent toolu_a · explorer · finished · claude-haiku-5-5\nFind the greeting\n\n▸ to the subagent\nWhere is hello?\n\n▸ tool (failed)\nread_file hello.rs\nNo such file\n\n▸ subagent\nxxx"
        ));
        assert!(text.ends_with("x\n… 3 more characters"));
    }
}
