//! Command-line flags: which session to open, and whether to run one
//! prompt without the terminal UI.

/// The parsed command line.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Args {
    pub(crate) launch: Launch,
    /// `-p TEXT`: run TEXT as one turn and print JSON Lines; `-` reads the
    /// prompt from standard input.
    pub(crate) print: Option<String>,
}

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
        "Usage: {program} [-c | -r [ID]] [-p PROMPT]\n\
         \n\
         Starts a new conversation in the current directory.\n\
         \n\
         Options:\n  \
           -c, --continue     Continue the most recently saved conversation\n  \
           -r, --resume [ID]  Resume a conversation; without ID, choose from a list.\n                     \
                              ID may be any unique prefix.\n  \
           -p, --print PROMPT Run PROMPT as one turn without the terminal UI and\n                     \
                              print its progress and result as JSON Lines.\n                     \
                              PROMPT - reads it from standard input. Calls that\n                     \
                              need approval are refused.\n  \
           -h, --help         Show this help"
    )
}

pub(crate) fn parse(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut args = args.into_iter().peekable();
    let mut launch = None;
    let mut print = None;
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
            "-p" | "--print" => {
                let prompt = args
                    .next()
                    .ok_or_else(|| format!("{arg} needs a prompt, or - to read one from stdin"))?;
                if print.replace(prompt).is_some() {
                    return Err("--print can be given only once".to_string());
                }
            }
            _ => match arg.strip_prefix("--resume=") {
                Some(id) if !id.is_empty() => {
                    set(Launch::Resume(Some(id.to_string())), &mut launch)?
                }
                _ => return Err(format!("Unexpected argument '{arg}'")),
            },
        }
    }
    let launch = launch.unwrap_or(Launch::New);
    if print.is_some() && launch == Launch::Resume(None) {
        return Err("--print cannot open the session picker; give --resume an ID".to_string());
    }
    Ok(Args { launch, print })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(args: &[&str]) -> Result<Launch, String> {
        parse(args.iter().map(|arg| arg.to_string())).map(|args| args.launch)
    }

    fn printed(args: &[&str]) -> Result<Args, String> {
        parse(args.iter().map(|arg| arg.to_string()))
    }

    #[test]
    fn print_runs_one_prompt_in_any_session_but_the_picker() {
        assert_eq!(
            printed(&["-p", "fix the test"]),
            Ok(Args {
                launch: Launch::New,
                print: Some("fix the test".into())
            })
        );
        assert_eq!(
            printed(&["-c", "--print", "-"]),
            Ok(Args {
                launch: Launch::Continue,
                print: Some("-".into())
            })
        );
        assert_eq!(
            printed(&["-r", "2026", "-p", "go on"]).unwrap().launch,
            Launch::Resume(Some("2026".into()))
        );
        assert!(printed(&["-p"]).unwrap_err().contains("needs a prompt"));
        assert!(printed(&["-r", "-p", "x"]).unwrap_err().contains("picker"));
        assert!(printed(&["-p", "a", "-p", "b"]).is_err());
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
