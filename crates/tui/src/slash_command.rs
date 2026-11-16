//! Slash command registry shared by dispatch, the completion popup, and help.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SlashAction {
    Help,
    Commands,
    Clear,
    Compact,
    Model,
    Resume,
    Rename,
    Rewind,
    Agents,
    Undo,
    Exit,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SlashCommand {
    pub(crate) name: &'static str,
    pub(crate) description: &'static str,
    pub(crate) action: SlashAction,
    /// Whether text may follow the name, as in `/model opus`.
    pub(crate) takes_argument: bool,
}

pub(crate) const SLASH_COMMANDS: &[SlashCommand] = &[
    SlashCommand {
        name: "help",
        description: "Show commands, key bindings, and sandbox policy",
        action: SlashAction::Help,
        takes_argument: false,
    },
    SlashCommand {
        name: "commands",
        description: "List available slash commands",
        action: SlashAction::Commands,
        takes_argument: false,
    },
    SlashCommand {
        name: "clear",
        description: "Start a new conversation; the current one stays resumable",
        action: SlashAction::Clear,
        takes_argument: false,
    },
    SlashCommand {
        name: "compact",
        description: "Summarize old conversation context to free tokens",
        action: SlashAction::Compact,
        takes_argument: false,
    },
    SlashCommand {
        name: "model",
        description: "Choose the model; /model <alias> switches directly",
        action: SlashAction::Model,
        takes_argument: true,
    },
    SlashCommand {
        name: "resume",
        description: "Resume another saved conversation; /resume <ID> opens one directly",
        action: SlashAction::Resume,
        takes_argument: true,
    },
    SlashCommand {
        name: "rename",
        description: "Name this conversation so -r <name> resumes it",
        action: SlashAction::Rename,
        takes_argument: true,
    },
    SlashCommand {
        name: "rewind",
        description: "Go back to an earlier prompt to edit it, and restore the files Socium changed since",
        action: SlashAction::Rewind,
        takes_argument: false,
    },
    SlashCommand {
        name: "agents",
        description: "List this session's subagents; /agents <id> shows one's conversation",
        action: SlashAction::Agents,
        takes_argument: true,
    },
    SlashCommand {
        name: "undo",
        description: "Revert Socium's last edit; /undo <path> reverts its last edit of that file",
        action: SlashAction::Undo,
        takes_argument: true,
    },
    SlashCommand {
        name: "exit",
        description: "Quit Socium",
        action: SlashAction::Exit,
        takes_argument: false,
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

/// Resolves a submitted command and its argument. A bare `exit` also quits,
/// as in Claude Code. Text after a command that takes no argument makes the
/// input an ordinary message.
pub(crate) fn find(input: &str) -> Option<(&'static SlashCommand, Option<&str>)> {
    let (token, argument) = if input == "exit" {
        ("exit", None)
    } else {
        let rest = input.strip_prefix('/')?;
        match rest.split_once(char::is_whitespace) {
            Some((token, argument)) => (token, Some(argument.trim())),
            None => (rest, None),
        }
    };
    let command = SLASH_COMMANDS
        .iter()
        .find(|command| command.name == token)?;
    match argument.filter(|argument| !argument.is_empty()) {
        Some(_) if !command.takes_argument => None,
        argument => Some((command, argument)),
    }
}

/// Length in chars of a fully typed command name at the start of `input`,
/// such as `/model` in `/model glm`, so the composer can highlight it. Text
/// after a command that takes no argument makes the input a message, so
/// nothing is highlighted then.
pub(crate) fn typed_command_len(input: &str) -> Option<usize> {
    let rest = input.strip_prefix('/')?;
    let (name, argument) = match rest.split_once(char::is_whitespace) {
        Some((name, argument)) => (name, argument.trim()),
        None => (rest, ""),
    };
    let command = SLASH_COMMANDS.iter().find(|command| command.name == name)?;
    (argument.is_empty() || command.takes_argument).then(|| 1 + name.chars().count())
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
        assert_eq!(names("/m"), ["model"]);
        assert_eq!(names("/re"), ["resume", "rename", "rewind"]);
        assert_eq!(names("/rew"), ["rewind"]);
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
        let action = |input| find(input).map(|(command, _)| command.action);
        assert_eq!(action("/exit"), Some(SlashAction::Exit));
        assert_eq!(action("exit"), Some(SlashAction::Exit));
        assert!(find("/quit").is_none());
        assert!(find("exit now").is_none());
        assert!(find("clear").is_none());
        assert_eq!(action("/clear"), Some(SlashAction::Clear));
        assert!(find("/cle").is_none());
        assert!(find("/clear now").is_none());
    }

    #[test]
    fn only_a_complete_command_name_is_highlighted() {
        assert_eq!(typed_command_len("/model"), Some(6));
        assert_eq!(typed_command_len("/model glm"), Some(6));
        assert_eq!(typed_command_len("/clear"), Some(6));
        assert_eq!(typed_command_len("/mod"), None);
        assert_eq!(typed_command_len("/models"), None);
        assert_eq!(typed_command_len("/clear now"), None);
        assert_eq!(typed_command_len("model"), None);
        assert_eq!(typed_command_len("/usr/bin is missing"), None);
    }

    #[test]
    fn model_takes_an_optional_alias() {
        let parsed = |input| find(input).map(|(command, argument)| (command.action, argument));
        assert_eq!(parsed("/model"), Some((SlashAction::Model, None)));
        assert_eq!(parsed("/model   "), Some((SlashAction::Model, None)));
        assert_eq!(
            parsed("/model  ds-flash "),
            Some((SlashAction::Model, Some("ds-flash")))
        );
        assert!(find("/models").is_none());
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
