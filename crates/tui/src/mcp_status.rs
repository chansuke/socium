//! Text for `/mcp`: every configured MCP server and how it is doing.

use socium_core::{McpDisabled, McpServerState, McpServerStatus, McpServerTransport};

/// One line per server, `name · local|remote · where · state`, with its
/// tools, the login hint, or the reason under it. The transcript wraps
/// system text by words, so columns would not stay aligned; the separators
/// keep the fields apart instead, as in `/agents`.
pub(crate) fn servers_text(servers: &[McpServerStatus]) -> String {
    if servers.is_empty() {
        return "No MCP servers are configured. Add them under [mcp.servers] in config.toml; \
                see the MCP servers section of docs/configuration.md."
            .to_string();
    }
    let mut lines = vec![
        "MCP servers (change them in [mcp.servers] of config.toml, then restart Socium):"
            .to_string(),
    ];
    for server in servers {
        let (kind, place) = match &server.transport {
            McpServerTransport::Local { command } => ("local", command_line(command)),
            McpServerTransport::Remote { url } => ("remote", url.clone()),
        };
        lines.push(String::new());
        lines.push(format!(
            "{} · {kind} · {place} · {}",
            server.name,
            state_label(&server.state)
        ));
        let detail = match &server.state {
            McpServerState::Connected { tools } if !tools.is_empty() => {
                format!("tools: {}", tools.join(", "))
            }
            McpServerState::NeedsLogin => format!(
                "run socium mcp login {} in a shell, then restart Socium",
                server.name
            ),
            McpServerState::Failed { reason } => reason.clone(),
            McpServerState::Disabled {
                reason: McpDisabled::UnsetVariable { setting, variable },
            } => format!(
                "{setting} uses ${{{variable}}}, which is not set; set it, then restart Socium"
            ),
            _ => continue,
        };
        lines.extend(detail.lines().map(|line| format!("  {line}")));
    }
    lines.join("\n")
}

fn state_label(state: &McpServerState) -> String {
    match state {
        McpServerState::Starting => "starting".to_string(),
        McpServerState::Connected { tools } if tools.len() == 1 => "connected, 1 tool".to_string(),
        McpServerState::Connected { tools } => format!("connected, {} tools", tools.len()),
        McpServerState::NeedsLogin => "needs login".to_string(),
        McpServerState::Failed { .. } => "failed".to_string(),
        McpServerState::Disabled {
            reason: McpDisabled::Setting,
        } => "disabled (enabled = false)".to_string(),
        McpServerState::Disabled {
            reason: McpDisabled::UnsetVariable { .. },
        } => "disabled (variable not set)".to_string(),
    }
}

/// The program and its arguments as one line, an argument with spaces or
/// quotes in single quotes.
fn command_line(argv: &[String]) -> String {
    argv.iter()
        .map(|arg| {
            if !arg.is_empty() && !arg.contains(|c: char| c.is_whitespace() || "'\"".contains(c)) {
                arg.clone()
            } else {
                format!("'{}'", arg.replace('\'', r"'\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(name: &str, command: &[&str], state: McpServerState) -> McpServerStatus {
        McpServerStatus {
            name: name.into(),
            transport: McpServerTransport::Local {
                command: command.iter().map(|arg| arg.to_string()).collect(),
            },
            state,
        }
    }

    fn remote(name: &str, url: &str, state: McpServerState) -> McpServerStatus {
        McpServerStatus {
            name: name.into(),
            transport: McpServerTransport::Remote { url: url.into() },
            state,
        }
    }

    #[test]
    fn every_state_is_one_line_with_its_details_under_it() {
        let text = servers_text(&[
            remote(
                "deepwiki",
                "https://mcp.deepwiki.com/mcp",
                McpServerState::Connected {
                    tools: vec!["ask_question".into()],
                },
            ),
            local(
                "gh",
                &["gh-mcp"],
                McpServerState::Disabled {
                    reason: McpDisabled::UnsetVariable {
                        setting: "env.TOKEN".into(),
                        variable: "GITHUB_TOKEN".into(),
                    },
                },
            ),
            remote(
                "linear",
                "https://mcp.linear.app/mcp",
                McpServerState::NeedsLogin,
            ),
            local(
                "missing",
                &["no-such-mcp"],
                McpServerState::Failed {
                    reason: "it stopped. Its last standard error lines:\nboom\nbye".into(),
                },
            ),
            local(
                "off",
                &["off-mcp"],
                McpServerState::Disabled {
                    reason: McpDisabled::Setting,
                },
            ),
            local("slow", &["slow-mcp"], McpServerState::Starting),
            local(
                "time",
                &["uvx", "mcp-server-time", "--local-timezone=Asia/Tokyo"],
                McpServerState::Connected {
                    tools: vec!["get_current_time".into(), "convert_time".into()],
                },
            ),
            local(
                "quiet",
                &["quiet-mcp"],
                McpServerState::Connected { tools: Vec::new() },
            ),
        ]);

        assert_eq!(
            text,
            "\
MCP servers (change them in [mcp.servers] of config.toml, then restart Socium):

deepwiki · remote · https://mcp.deepwiki.com/mcp · connected, 1 tool
  tools: ask_question

gh · local · gh-mcp · disabled (variable not set)
  env.TOKEN uses ${GITHUB_TOKEN}, which is not set; set it, then restart Socium

linear · remote · https://mcp.linear.app/mcp · needs login
  run socium mcp login linear in a shell, then restart Socium

missing · local · no-such-mcp · failed
  it stopped. Its last standard error lines:
  boom
  bye

off · local · off-mcp · disabled (enabled = false)

slow · local · slow-mcp · starting

time · local · uvx mcp-server-time --local-timezone=Asia/Tokyo · connected, 2 tools
  tools: get_current_time, convert_time

quiet · local · quiet-mcp · connected, 0 tools"
        );
    }

    #[test]
    fn arguments_with_spaces_or_quotes_are_quoted() {
        assert_eq!(
            command_line(&[
                "node".into(),
                "my server.js".into(),
                "it's".into(),
                "".into()
            ]),
            r"node 'my server.js' 'it'\''s' ''"
        );
    }

    #[test]
    fn no_servers_points_to_the_configuration_guide() {
        let text = servers_text(&[]);

        assert!(text.starts_with("No MCP servers are configured."), "{text}");
        assert!(text.contains("docs/configuration.md"), "{text}");
    }
}
