//! Slash command registry shared by dispatch, the completion popup, and help.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SlashAction {
    Help,
    Commands,
    Clear,
    Compact,
    Exit,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SlashCommand {
    pub(crate) name: &'static str,
    pub(crate) description: &'static str,
    pub(crate) action: SlashAction,
}

pub(crate) const SLASH_COMMANDS: &[SlashCommand] = &[
    SlashCommand {
        name: "help",
        description: "Show commands, key bindings, and sandbox policy",
        action: SlashAction::Help,
    },
    SlashCommand {
        name: "commands",
        description: "List available slash commands",
        action: SlashAction::Commands,
    },
    SlashCommand {
        name: "clear",
        description: "Clear the current conversation",
        action: SlashAction::Clear,
    },
    SlashCommand {
        name: "compact",
        description: "Summarize old conversation context to free tokens",
        action: SlashAction::Compact,
    },
    SlashCommand {
        name: "exit",
        description: "Quit Socium",
        action: SlashAction::Exit,
    },
    SlashCommand {
        name: "quit",
        description: "Quit Socium",
        action: SlashAction::Exit,
    },
];

/// Returns the slash command token being typed, or `None` once the input is
/// no longer a bare `/name` (for example a message that starts with a path).
pub(crate) fn command_token(input: &str) -> Option<&str> {
    let token = input.strip_prefix('/')?;
    (!token.chars().any(char::is_whitespace)).then_some(token)
}

pub(crate) fn matching_commands(input: &str) -> Vec<&'static SlashCommand> {
    let Some(token) = command_token(input) else {
        return Vec::new();
    };
    SLASH_COMMANDS
        .iter()
        .filter(|command| command.name.starts_with(token))
        .collect()
}

pub(crate) fn find(input: &str) -> Option<&'static SlashCommand> {
    let token = command_token(input)?;
    SLASH_COMMANDS.iter().find(|command| command.name == token)
}

pub(crate) fn command_list_text() -> String {
    let width = SLASH_COMMANDS
        .iter()
        .map(|command| command.name.len() + 1)
        .max()
        .unwrap_or(0);
    SLASH_COMMANDS
        .iter()
        .map(|command| {
            format!(
                "{:<width$}  {}",
                format!("/{}", command.name),
                command.description
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(input: &str) -> Vec<&'static str> {
        matching_commands(input)
            .into_iter()
            .map(|command| command.name)
            .collect()
    }

    #[test]
    fn slash_alone_lists_every_command_in_registry_order() {
        let expected = SLASH_COMMANDS
            .iter()
            .map(|command| command.name)
            .collect::<Vec<_>>();
        assert_eq!(names("/"), expected);
    }

    #[test]
    fn prefix_narrows_candidates() {
        assert_eq!(names("/c"), ["commands", "clear", "compact"]);
        assert_eq!(names("/co"), ["commands", "compact"]);
        assert_eq!(names("/cl"), ["clear"]);
        assert!(names("/zzz").is_empty());
    }

    #[test]
    fn non_command_input_has_no_candidates() {
        assert!(names("").is_empty());
        assert!(names("help").is_empty());
        assert!(names("/help me").is_empty());
        assert!(names("/usr/bin is missing").is_empty());
    }

    #[test]
    fn find_requires_an_exact_name() {
        assert_eq!(find("/quit").map(|c| c.action), Some(SlashAction::Exit));
        assert_eq!(find("/clear").map(|c| c.action), Some(SlashAction::Clear));
        assert!(find("/cle").is_none());
        assert!(find("/clear now").is_none());
    }

    #[test]
    fn command_list_aligns_every_command_with_its_description() {
        let list = command_list_text();
        let lines = list.lines().collect::<Vec<_>>();

        assert_eq!(lines.len(), SLASH_COMMANDS.len());
        assert_eq!(
            lines[0],
            "/help      Show commands, key bindings, and sandbox policy"
        );
        assert!(lines.contains(&"/compact   Summarize old conversation context to free tokens"));
    }
}
