//! Command-line flags: which session to open, and whether to run one
//! prompt without the terminal UI; or the `models import` or `mcp`
//! subcommand.

/// The parsed command line.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Args {
    pub(crate) launch: Launch,
    /// `-p`: run one turn without the terminal UI.
    pub(crate) print: Option<Print>,
    /// `--dangerously-skip-permissions`: no tool call asks for approval.
    pub(crate) skip_permissions: bool,
}

/// `-p [PROMPT] [--output-format FMT]`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Print {
    /// The positional prompt; piped standard input comes before it.
    pub(crate) prompt: Option<String>,
    pub(crate) format: OutputFormat,
}

/// What `-p` writes to standard output.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputFormat {
    /// The reply text alone.
    #[default]
    Text,
    /// The `finished` record alone.
    Json,
    /// Every record as JSON Lines.
    StreamJson,
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
    Version,
    /// `models import [--from FILE] [PROVIDER...]`.
    ModelsImport(ModelsImport),
    /// `mcp login|logout [--] SERVER`, which opens no session.
    Mcp(McpCommand),
}

/// Writes registry files for models.dev providers, or lists the providers.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ModelsImport {
    /// Catalog provider IDs; none lists them instead.
    pub(crate) providers: Vec<String>,
    /// A saved copy of the catalog to read instead of downloading it.
    pub(crate) from: Option<String>,
}

/// `socium mcp ...`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum McpCommand {
    /// Log in to the remote server with OAuth and keep its token.
    Login(String),
    /// Forget the server's token.
    Logout(String),
}

pub(crate) fn usage(program: &str) -> String {
    format!(
        "Usage: {program} [-c | -r [ID]] [-p [PROMPT] [--output-format FMT]]\n       \
                {program} models import [--from FILE] [PROVIDER...]\n       \
                {program} mcp login|logout [--] SERVER\n\
         \n\
         Starts a new conversation in the current directory.\n\
         \n\
         models import writes models.d/PROVIDER.toml in the config directory for\n\
         each models.dev provider, from https://models.dev/api.json or a saved copy\n\
         (--from FILE). Without PROVIDER it lists the providers. Review the base\n\
         URLs it prints: API keys are sent there.\n\
         \n\
         Options:\n  \
           -c, --continue     Continue the most recently saved conversation\n  \
           -r, --resume [ID]  Resume a conversation; without ID, choose from a list.\n                     \
                              ID may be any unique prefix.\n  \
           -p, --print        Run PROMPT as one turn without the terminal UI and\n                     \
                              print the reply. Piped standard input comes before\n                     \
                              PROMPT. Calls that need approval are refused,\n                     \
                              unless --dangerously-skip-permissions is given.\n      \
               --output-format FMT\n                     \
                              With -p: text (the reply, the default), json (the\n                     \
                              final result), or stream-json (progress and result\n                     \
                              as JSON Lines)\n      \
               --dangerously-skip-permissions\n                     \
                              Run every tool call without asking, commits and\n                     \
                              edits to files host tools run included. Deny\n                     \
                              rules and the sandbox still apply.\n  \
           -h, --help         Show this help\n  \
           -V, --version      Show the version\n\
         \n\
         Commands:\n  \
           mcp login SERVER   Log in to the remote MCP server SERVER of config.toml\n                     \
                              with OAuth in a browser, and keep its token for\n                     \
                              later runs\n  \
           mcp logout SERVER  Forget the stored token of SERVER\n\
         \n\
         Put -- before a SERVER whose name begins with '-'."
    )
}

pub(crate) fn parse(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let args = args.into_iter().collect::<Vec<_>>();
    // Asked to identify itself, it does so whatever else is on the line,
    // up to a `--` after which every argument is a name.
    if args
        .iter()
        .take_while(|arg| *arg != "--")
        .any(|arg| arg == "-V" || arg == "--version")
    {
        return Ok(Args {
            launch: Launch::Version,
            print: None,
            skip_permissions: false,
        });
    }
    if args.first().is_some_and(|arg| arg == "models") {
        return parse_models(&args[1..]).map(|launch| Args {
            launch,
            print: None,
            skip_permissions: false,
        });
    }
    let mut args = args.into_iter().peekable();
    if args.next_if(|first| first == "mcp").is_some() {
        return mcp(args.collect()).map(|launch| Args {
            launch,
            print: None,
            skip_permissions: false,
        });
    }
    let mut launch = None;
    let mut print = false;
    let mut prompt = None;
    let mut format = None;
    let mut skip_permissions = false;
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
            "-p" | "--print" => print = true,
            "--dangerously-skip-permissions" => skip_permissions = true,
            "--output-format" => {
                let value = args
                    .next()
                    .ok_or("--output-format needs text, json, or stream-json")?;
                format = Some(output_format(&value)?);
            }
            _ if !arg.starts_with('-') && prompt.is_none() => prompt = Some(arg),
            _ => {
                if let Some(id) = arg.strip_prefix("--resume=").filter(|id| !id.is_empty()) {
                    set(Launch::Resume(Some(id.to_string())), &mut launch)?;
                } else if let Some(value) = arg.strip_prefix("--output-format=") {
                    format = Some(output_format(value)?);
                } else {
                    return Err(format!("Unexpected argument '{arg}'"));
                }
            }
        }
    }
    let launch = launch.unwrap_or(Launch::New);
    if !print {
        if let Some(prompt) = prompt {
            return Err(format!("Unexpected argument '{prompt}'"));
        }
        if format.is_some() {
            return Err("--output-format needs --print".to_string());
        }
        return Ok(Args {
            launch,
            print: None,
            skip_permissions,
        });
    }
    if launch == Launch::Resume(None) {
        return Err("--print cannot open the session picker; give --resume an ID".to_string());
    }
    let format = format.unwrap_or_default();
    Ok(Args {
        launch,
        print: Some(Print { prompt, format }),
        skip_permissions,
    })
}

/// `models import [--from FILE] [PROVIDER...]`, after `models`.
fn parse_models(args: &[String]) -> Result<Launch, String> {
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        return Ok(Launch::Help);
    }
    let Some((subcommand, rest)) = args.split_first() else {
        return Err("models needs a subcommand: import".into());
    };
    if subcommand != "import" {
        return Err(format!(
            "Unknown models subcommand '{subcommand}'; use import"
        ));
    }
    let mut import = ModelsImport {
        providers: Vec::new(),
        from: None,
    };
    let mut rest = rest.iter();
    while let Some(arg) = rest.next() {
        let from = match arg.as_str() {
            "--from" => Some(rest.next().ok_or("--from needs a file")?.as_str()),
            _ => arg.strip_prefix("--from="),
        };
        match from {
            Some("") => return Err("--from needs a file".into()),
            Some(_) if import.from.is_some() => return Err("--from is given twice".into()),
            Some(path) => import.from = Some(path.to_string()),
            None if arg.starts_with('-') => return Err(format!("Unexpected argument '{arg}'")),
            None if !import.providers.contains(arg) => import.providers.push(arg.clone()),
            None => {}
        }
    }
    Ok(Launch::ModelsImport(import))
}

/// `mcp login|logout [--] SERVER`, or the help. A server name may begin
/// with `-`, as config.toml allows; after `--` it is not taken for a flag.
fn mcp(args: Vec<String>) -> Result<Launch, String> {
    let args = args.iter().map(String::as_str).collect::<Vec<_>>();
    let command = |verb: &str, server: &str| match verb {
        "login" => Launch::Mcp(McpCommand::Login(server.into())),
        _ => Launch::Mcp(McpCommand::Logout(server.into())),
    };
    match args[..] {
        [verb @ ("login" | "logout"), "--", server] => Ok(command(verb, server)),
        [verb @ ("login" | "logout"), server] if !server.starts_with('-') => {
            Ok(command(verb, server))
        }
        _ if args
            .iter()
            .take_while(|arg| **arg != "--")
            .any(|arg| matches!(*arg, "-h" | "--help")) =>
        {
            Ok(Launch::Help)
        }
        ["login" | "logout"] | ["login" | "logout", "--"] => {
            Err(format!("mcp {} needs a server name", args[0]))
        }
        _ => Err("mcp takes login SERVER or logout SERVER".to_string()),
    }
}

fn output_format(value: &str) -> Result<OutputFormat, String> {
    match value {
        "text" => Ok(OutputFormat::Text),
        "json" => Ok(OutputFormat::Json),
        "stream-json" => Ok(OutputFormat::StreamJson),
        _ => Err(format!(
            "Unknown output format '{value}'; use text, json, or stream-json"
        )),
    }
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

    fn print(prompt: Option<&str>, format: OutputFormat) -> Option<Print> {
        Some(Print {
            prompt: prompt.map(str::to_string),
            format,
        })
    }

    #[test]
    fn version_wins_over_other_flags() {
        for args in [
            &["--version"][..],
            &["-V"],
            &["-c", "--version"],
            &["-p", "q", "-V"],
            &["--help", "--version"],
            &["--version", "--output-format", "json"],
            &["-c", "-r", "--version"],
            &["--version", "--bogus"],
        ] {
            assert_eq!(printed(args).unwrap().launch, Launch::Version, "{args:?}");
        }
    }

    #[test]
    fn print_takes_the_prompt_as_a_positional_anywhere() {
        let new = |print| {
            Ok(Args {
                launch: Launch::New,
                print,
                skip_permissions: false,
            })
        };
        assert_eq!(
            printed(&["-p", "fix the test"]),
            new(print(Some("fix the test"), OutputFormat::Text))
        );
        assert_eq!(
            printed(&["fix the test", "--print"]),
            new(print(Some("fix the test"), OutputFormat::Text))
        );
        assert_eq!(printed(&["-p"]), new(print(None, OutputFormat::Text)));
        assert_eq!(
            printed(&["-c", "-p", "go on"]),
            Ok(Args {
                launch: Launch::Continue,
                print: print(Some("go on"), OutputFormat::Text),
                skip_permissions: false,
            })
        );
        assert_eq!(
            printed(&["-r", "2026", "-p", "go on"]).unwrap().launch,
            Launch::Resume(Some("2026".into()))
        );
        assert!(printed(&["-r", "-p", "x"]).unwrap_err().contains("picker"));
        assert!(printed(&["-p", "a", "b"]).unwrap_err().contains("'b'"));
        assert_eq!(printed(&["-p", "-"]), Err("Unexpected argument '-'".into()));
        assert_eq!(printed(&["q"]), Err("Unexpected argument 'q'".into()));
    }

    #[test]
    fn output_format_picks_what_print_writes() {
        let format = |args: &[&str]| printed(args).map(|args| args.print.unwrap().format);
        assert_eq!(
            format(&["-p", "q", "--output-format", "text"]),
            Ok(OutputFormat::Text)
        );
        assert_eq!(
            format(&["--output-format=json", "-p", "q"]),
            Ok(OutputFormat::Json)
        );
        assert_eq!(
            format(&["-p", "--output-format", "stream-json", "q"]),
            Ok(OutputFormat::StreamJson)
        );
        assert!(
            format(&["-p", "--output-format", "xml"])
                .unwrap_err()
                .contains("'xml'")
        );
        assert!(format(&["-p", "--output-format"]).is_err());
        assert!(
            printed(&["--output-format", "json"])
                .unwrap_err()
                .contains("--print")
        );
    }

    #[test]
    fn skipping_permissions_works_with_and_without_print() {
        let skips = |args: &[&str]| printed(args).map(|args| args.skip_permissions);
        assert_eq!(skips(&[]), Ok(false));
        assert_eq!(skips(&["--dangerously-skip-permissions"]), Ok(true));
        assert_eq!(skips(&["-c", "--dangerously-skip-permissions"]), Ok(true));
        assert_eq!(
            skips(&["-p", "q", "--dangerously-skip-permissions"]),
            Ok(true)
        );
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
    fn models_import_takes_provider_ids_and_a_saved_catalog() {
        let import = |providers: &[&str], from: Option<&str>| {
            Ok(Launch::ModelsImport(ModelsImport {
                providers: providers.iter().map(|id| id.to_string()).collect(),
                from: from.map(str::to_string),
            }))
        };
        assert_eq!(parsed(&["models", "import"]), import(&[], None));
        assert_eq!(
            parsed(&["models", "import", "opencode", "anthropic", "opencode"]),
            import(&["opencode", "anthropic"], None)
        );
        assert_eq!(
            parsed(&["models", "import", "--from", "api.json", "groq"]),
            import(&["groq"], Some("api.json"))
        );
        assert_eq!(
            parsed(&["models", "import", "groq", "--from=api.json"]),
            import(&["groq"], Some("api.json"))
        );
        assert_eq!(parsed(&["models", "import", "-h"]), Ok(Launch::Help));
        assert_eq!(parsed(&["models", "-V"]), Ok(Launch::Version));
        for (args, error) in [
            (&["models"][..], "needs a subcommand"),
            (&["models", "list"], "Unknown models subcommand 'list'"),
            (&["models", "import", "--from"], "--from needs a file"),
            (&["models", "import", "--from="], "--from needs a file"),
            (&["models", "import", "--from", "a", "--from", "b"], "twice"),
            (&["models", "import", "-c"], "Unexpected argument '-c'"),
        ] {
            let err = parsed(args).unwrap_err();
            assert!(err.contains(error), "{args:?}: {err}");
        }
        // Only the first word selects the subcommand; elsewhere it is a prompt.
        assert_eq!(
            printed(&["-p", "models"]).unwrap().print.unwrap().prompt,
            Some("models".into())
        );
    }

    #[test]
    fn mcp_logs_in_to_or_out_of_one_server() {
        let mcp = |command| {
            Ok(Args {
                launch: Launch::Mcp(command),
                print: None,
                skip_permissions: false,
            })
        };
        assert_eq!(
            printed(&["mcp", "login", "github"]),
            mcp(McpCommand::Login("github".into()))
        );
        assert_eq!(
            printed(&["mcp", "logout", "github"]),
            mcp(McpCommand::Logout("github".into()))
        );
        assert_eq!(parsed(&["mcp", "--help"]), Ok(Launch::Help));
        assert_eq!(parsed(&["mcp", "login", "-h"]), Ok(Launch::Help));
        assert_eq!(
            parsed(&["mcp", "login"]),
            Err("mcp login needs a server name".into())
        );
        for bad in [
            &["mcp"][..],
            &["mcp", "list"],
            &["mcp", "login", "a", "b"],
            &["mcp", "login", "-p"],
        ] {
            assert!(parsed(bad).is_err(), "{bad:?}");
        }
        // Only as the first argument.
        assert!(parsed(&["-c", "mcp", "login", "x"]).is_err());
    }

    #[test]
    fn a_server_name_that_begins_with_a_dash_follows_a_double_dash() {
        let mcp = |command| Ok(Launch::Mcp(command));
        assert_eq!(
            parsed(&["mcp", "login", "--", "-gh"]),
            mcp(McpCommand::Login("-gh".into()))
        );
        assert_eq!(
            parsed(&["mcp", "logout", "--", "-gh"]),
            mcp(McpCommand::Logout("-gh".into()))
        );
        // After `--`, neither -h nor -V is a flag.
        assert_eq!(
            parsed(&["mcp", "login", "--", "-h"]),
            mcp(McpCommand::Login("-h".into()))
        );
        assert_eq!(
            parsed(&["mcp", "logout", "--", "-V"]),
            mcp(McpCommand::Logout("-V".into()))
        );
        assert_eq!(
            parsed(&["mcp", "login", "--", "github"]),
            mcp(McpCommand::Login("github".into()))
        );
        assert_eq!(
            parsed(&["mcp", "login", "--"]),
            Err("mcp login needs a server name".into())
        );
        for bad in [
            &["mcp", "login", "-gh"][..],
            &["mcp", "login", "--", "a", "b"],
            &["mcp", "--", "login", "x"],
        ] {
            assert!(parsed(bad).is_err(), "{bad:?}");
        }
        assert!(usage("socium").contains("mcp login|logout [--] SERVER"));
    }

    #[test]
    fn models_and_mcp_take_each_other_s_name_as_an_argument() {
        assert_eq!(
            parsed(&["models", "import", "mcp"]),
            Ok(Launch::ModelsImport(ModelsImport {
                providers: vec!["mcp".into()],
                from: None,
            }))
        );
        assert_eq!(
            parsed(&["mcp", "login", "models"]),
            Ok(Launch::Mcp(McpCommand::Login("models".into())))
        );
        for subcommand in ["models", "mcp"] {
            assert!(
                parsed(&[subcommand, "--dangerously-skip-permissions"]).is_err(),
                "{subcommand}"
            );
        }
        let usage = usage("socium");
        assert!(usage.contains("socium models import [--from FILE] [PROVIDER...]\n"));
        assert!(usage.contains("       socium mcp login|logout [--] SERVER\n"));
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
