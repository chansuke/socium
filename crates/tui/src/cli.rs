//! Command-line flags: which session to open.

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Launch {
    New,
    /// `-c`: the most recently saved session.
    Continue,
    /// `-r [ID]`: the session with this ID or prefix, or a picker without one.
    Resume(Option<String>),
    Help,
}

pub(crate) fn usage(program: &str) -> String {
    format!(
        "Usage: {program} [-c | -r [ID]]\n\
         \n\
         Starts a new conversation in the current directory.\n\
         \n\
         Options:\n  \
           -c, --continue     Continue the most recently saved conversation\n  \
           -r, --resume [ID]  Resume a conversation; without ID, choose from a list.\n                     \
                              ID may be any unique prefix.\n  \
           -h, --help         Show this help"
    )
}

pub(crate) fn parse(args: impl IntoIterator<Item = String>) -> Result<Launch, String> {
    let mut args = args.into_iter().peekable();
    let mut launch = None;
    let set = |next: Launch, launch: &mut Option<Launch>| match launch {
        Some(Launch::Help) => Ok(()),
        Some(_) if next != Launch::Help => {
            Err("--continue and --resume cannot be combined".to_string())
        }
        _ => {
            *launch = Some(next);
            Ok(())
        }
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => set(Launch::Help, &mut launch)?,
            "-c" | "--continue" => set(Launch::Continue, &mut launch)?,
            "-r" | "--resume" => {
                let id = args.next_if(|next| !next.starts_with('-'));
                set(Launch::Resume(id), &mut launch)?;
            }
            _ => match arg.strip_prefix("--resume=") {
                Some(id) if !id.is_empty() => {
                    set(Launch::Resume(Some(id.to_string())), &mut launch)?
                }
                _ => return Err(format!("Unexpected argument '{arg}'")),
            },
        }
    }
    Ok(launch.unwrap_or(Launch::New))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(args: &[&str]) -> Result<Launch, String> {
        parse(args.iter().map(|arg| arg.to_string()))
    }

    #[test]
    fn flags_choose_the_session() {
        assert_eq!(parsed(&[]), Ok(Launch::New));
        assert_eq!(parsed(&["-c"]), Ok(Launch::Continue));
        assert_eq!(parsed(&["--continue"]), Ok(Launch::Continue));
        assert_eq!(parsed(&["-r"]), Ok(Launch::Resume(None)));
        assert_eq!(
            parsed(&["-r", "20261114"]),
            Ok(Launch::Resume(Some("20261114".into())))
        );
        assert_eq!(
            parsed(&["--resume=20261114-1530"]),
            Ok(Launch::Resume(Some("20261114-1530".into())))
        );
        assert_eq!(parsed(&["--resume", "-h"]), Ok(Launch::Help));
        assert_eq!(parsed(&["-h", "-c"]), Ok(Launch::Help));
    }

    #[test]
    fn conflicting_or_unknown_arguments_are_refused() {
        assert!(
            parsed(&["-c", "-r"])
                .unwrap_err()
                .contains("cannot be combined")
        );
        assert!(parsed(&["-r", "a", "-c"]).is_err());
        assert_eq!(parsed(&["-x"]), Err("Unexpected argument '-x'".into()));
        assert!(parsed(&["-c", "extra"]).is_err());
        assert!(parsed(&["--resume="]).is_err());
    }
}
