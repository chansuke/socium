# Configuration

Socium reads environment variables at startup and two optional files in
`~/.config/socium/`: `models.toml` registers model providers, and
`config.toml` says what the sandbox may see and when a tool call needs your
approval. Neither file may lie inside a workspace, so the model can never
change it.

## Requirements

Execution requires Linux (x86_64 or aarch64), bubblewrap 0.12 or newer, and
unprivileged user namespaces. The TUI checks the sandbox at startup and refuses
to run if isolation cannot be established; there is no unsandboxed fallback,
and macOS and Windows are unsupported. Distribution packages older than 0.12.0
(for example Ubuntu's 0.9.0) are refused. On Ubuntu, bubblewrap also needs an
AppArmor `userns` profile; see the [Linux sandbox guide](linux-sandbox.md).

Install Git, ripgrep (`rg`), and bubblewrap (`bwrap`). The Linux Nix package
includes them on its runtime PATH. Socium's tools look for `rg` and `git` in
the system directories on PATH (`/usr`, `/bin`, `/nix/store`) and in `/usr/bin`
and `/bin`; when one is not there, startup names it and the tools that will
fail.

A project directory can be an ordinary checkout or a linked worktree made by
`git worktree add`; for a worktree, the main repository's Git directory is
mounted read-only (writable only during `git_commit`). Submodule gitfiles and
other redirected Git metadata are refused.

## Environment variables

Set `ANTHROPIC_API_KEY` before starting, or the key of another registered
provider. The default model is Claude Opus 5.5 (`claude-opus-5-5`).

| Variable | Effect |
| --- | --- |
| `MODEL` | Model alias or ID to start with. A name that is neither a registered alias nor a registered ID is sent as a Claude model ID to Anthropic. |
| `THINKING_DISPLAY` | What the API returns as text for adaptive thinking: `summarized` (default; reasoning summaries and progress updates), `updates` (progress updates between tool calls only), or `omitted`. |
| `EFFORT` | Thinking depth: `low`, `medium`, `high`, `xhigh`, or `max`. When unset the model's default applies. |
| `MAX_TOKENS` | Caps each response, thinking included. Defaults to 64000; a model's `max_output_tokens` in `models.toml` wins over it. |
| `CONTEXT_WINDOW_TOKENS` | Replaces every model's context window. |
| `COMPACTION_TOKEN_THRESHOLD` | Replaces the prompt size at which history is compacted. |
| `ANTHROPIC_VERSION` | The `anthropic-version` header (default `2023-06-01`). |
| `SOCIUM_MODELS_FILE`, `SOCIUM_CONFIG_FILE`, `SOCIUM_AGENTS_DIR` | Other locations for `models.toml`, `config.toml`, and the subagent definitions. |

A limit that is not a positive integer is ignored with a warning. An output
limit as large as the context window is reported at startup and on `/model`,
and stops a turn before any request. `THINKING_BUDGET_TOKENS` and
`COMPACT_MODEL` are no longer supported. Model names must be available to your
API account.

## Models

`/model` opens a numbered list of the registered models in place of the
composer, with a check on the current one; choose with the arrow keys and Enter,
or press its number. `/model <alias>` switches at once, and Tab completes the
alias. Switching is allowed between turns only and keeps the history whole:
each reply records the provider and model that wrote it, and thinking is sent
back only to a provider that can read it (Anthropic's API reads any Claude
model's thinking; other endpoints only their own model's).

The choice is saved with the conversation, so resuming it, or starting a new
conversation after it, uses the same model. At startup the saved choice (of the
resumed conversation, or of the most recently saved one for a new conversation)
wins over `MODEL`, which wins over the registry default; when none of them has
its API key set, Socium starts on the first registered model that does and says
so.

### Registering providers

The built-in registry has `opus`, `sonnet`, and `haiku` on Anthropic, each with
a 1M-token context window. Add other endpoints in
`~/.config/socium/models.toml` (or `$XDG_CONFIG_HOME/socium/models.toml`, or
the file named by `SOCIUM_MODELS_FILE`); start from
[`models.toml.example`](../models.toml.example). The file names the environment
variable that holds each key, never the key itself:

```toml
default = "opus"

[providers.deepseek]
wire = "anthropic"
base_url = "https://api.deepseek.com/anthropic"
api_key_env = "DEEPSEEK_API_KEY"
auth = "bearer"            # or "x-api-key" (default)

[providers.zai]
wire = "anthropic"
base_url = "https://api.z.ai/api/anthropic"
api_key_env = "ZAI_API_KEY"
auth = "bearer"

[[models]]
alias = "ds-flash"
provider = "deepseek"
id = "deepseek-flash"        # DeepSeek-V4.1-Flash; deepseek-v4-pro is the larger model
description = "DeepSeek-V4.1-Flash, fast and inexpensive"  # shown in /model
features = ["effort"]
context_window = 1000000
max_output_tokens = 32000
# compact_model = "ds-flash" # alias that summarizes history; defaults to this model

[[models]]
alias = "glm"
provider = "zai"
id = "glm-5.3"               # always reasons; 1M context, up to 128K output
context_window = 1000000
max_output_tokens = 32000
```

`base_url` must use HTTPS, except on localhost. Model IDs above were checked
against the DeepSeek and Z.ai documentation on 2026-11-13; DeepSeek rejects
unknown non-Claude names with 400.

`compact_at_tokens` sets the prompt size at which a model's history is
compacted (default 200000, or 80% of the input budget if that is lower), and
`price` gives its US dollar prices per million tokens so the status line can
show the session's cost; Socium ships no prices.

### Wire formats

- `wire = "anthropic"` speaks Anthropic Messages. Anthropic extensions
  (adaptive thinking, effort, eager tool input streaming, the built-in text
  editor tool type, and prompt caching) are sent only to models whose
  `features` list them; built-in Claude models list all five
  (`adaptive_thinking`, `effort`, `eager_input_streaming`,
  `anthropic_text_editor`, `prompt_caching`), and other models get none,
  receiving the text editor as an ordinary JSON-schema tool.
- `wire = "openai-chat"` speaks OpenAI Chat Completions: OpenAI itself
  (`https://api.openai.com/v1`), Ollama Cloud (`https://ollama.com/v1`),
  OpenRouter, or a local server such as Ollama (`http://localhost:11434/v1`).
  These providers use Bearer authentication, and only the `effort` feature
  applies, sent as `reasoning_effort`. Reasoning text that some servers stream
  is shown in the Activity pane but not kept in the history.
- `wire = "openai-responses"` speaks the OpenAI Responses API
  (`https://api.openai.com/v1`), which lets reasoning models such as GPT-6
  Astra call tools; Chat Completions rejects tools with reasoning on that
  model. The `effort` feature marks a reasoning model: its reasoning summaries
  are shown in the Activity pane, `EFFORT` is sent as `reasoning.effort`, and
  its encrypted reasoning is sent back with the next request so it carries
  across tool calls. Nothing is stored on the server (`store: false`).

To check a registered model against its real API (this costs money), run
`SOCIUM_LIVE_MODEL=<alias> nix develop -c cargo test -p socium-core live_ -- --ignored`.

## Sandbox and approval settings

`~/.config/socium/config.toml` (or `$XDG_CONFIG_HOME/socium/config.toml`, or the
file named by `SOCIUM_CONFIG_FILE`) says what commands may see beyond the
workspace and when a tool call needs your approval; start from
[`config.toml.example`](../config.toml.example). The file must lie outside every
workspace Socium opens, and startup lists what it adds.

- `[sandbox]`: `read_only` lists host directories commands may read but not
  change, such as `~/.cargo/registry` and `~/.rustup`; the home directory
  itself, `/`, and the sandbox's own mounts are refused. `path` adds directories
  inside them to PATH, `command_timeout` sets the longest a command may run
  (seconds, default 120), and `[sandbox.env]` sets variables such as
  `CARGO_HOME` (PATH, HOME, TMPDIR, and `GIT_*` are refused). Network access
  stays off.
- `[approval]`: with `commands = "ask"` (default) each command waits for you;
  you can allow it once or allow its command prefix (such as `cargo test`) for
  the rest of the session. With `commands = "auto"`, and for commands that match
  an `allow` prefix, commands run without asking in a narrower sandbox: files
  that hold secrets read as empty, and files the host runs on its own cannot
  change. `deny` prefixes are always refused. Commits always ask, with the diff.
- Edits to files a host tool runs or loads by itself (`build.rs`, `Cargo.toml`,
  `Cargo.lock`, `.envrc`, `flake.nix`, `.cargo/config.toml`, `.vscode/`, `.github/workflows/`, Git hook
  managers, and similar) always ask, and so do reads of files that commonly
  hold credentials (`.env`, `.netrc`, `*.pem`, SSH keys, `auth.json`,
  `credentials.json`, and similar); `protected`
  and `sensitive` replace these lists. Approval has no time limit. A denied call
  tells the model not to retry it.

See the [Linux sandbox guide](linux-sandbox.md) for the exact filesystem
policy, environment, toolchain requirements, and restrictions on networked
builds.

## Web search

Web search is off by default. With `[web_search] enabled = true` in
`config.toml` and `DEEPSEEK_API_KEY` set, the model gets a `web_search` tool
backed by DeepSeek's paid Search API
(https://api-docs.deepseek.com/guides/search/), whichever provider runs the
model. Each query the model writes is sent to DeepSeek and each search with
results is billed to that key; nothing from the workspace is added to the query.
Results come back with their URLs and are treated as web text, not as
instructions. Explorer subagents do not get the tool. Turned on without the
key, the tool is not offered and startup says why. To try it against the real
API (this costs money), run
`nix develop -c cargo test -p socium-core live_web_search -- --ignored`.

## MCP servers

Socium can start MCP (Model Context Protocol) servers, or connect to remote
ones, and offer their tools to the model. Only tools are used: no resources,
prompts, or sampling. Each server is a table under `[mcp.servers]` in
`config.toml`; a workspace cannot add one. A table has exactly one of
`command` (a local server over stdio) or `url` (a remote server over
Streamable HTTP).

```toml
[mcp.servers.github]
command = ["github-mcp-server", "stdio"]
env = { GITHUB_PERSONAL_ACCESS_TOKEN = "${GITHUB_TOKEN}" }
allow = ["get_issue"]
# enabled = false

[mcp.servers.deepwiki]
url = "https://mcp.deepwiki.com/mcp"
# headers = { Authorization = "Bearer ${DEEPWIKI_TOKEN}" }
allow = ["read_wiki_structure"]

[mcp.servers.linear]
url = "https://mcp.linear.app/mcp"
# Only for a client registered ahead of time:
# oauth = { client_id = "...", client_secret = "${LINEAR_CLIENT_SECRET}", scopes = ["read"], callback_port = 8976 }
```

- `command` is the program, an absolute path or a name found on PATH, and its
  arguments. It runs on the host, outside the sandbox, so it has your network
  and your files. Configure only servers you trust. Its directory is `/`, not
  the workspace, so a launcher such as `python3 -m` or `npx` does not pick up
  the workspace's modules or packages; give a server that works on a directory
  its absolute path.
- The server gets PATH (without relative entries or entries in the workspace),
  HOME, USER, and LANG from Socium's environment, plus `env`; nothing else, so
  provider API keys reach it only if `env` names them. `${NAME}` in an `env`
  value is replaced by that variable from Socium's environment. When it is not
  set, the server is not started and startup says why.
- `tool_timeout_secs` (default 600) is how long a tool call may take. A call
  that runs longer, or that you cancel, is given up and the server is asked to
  stop it; whether it ran is then unknown, and it is not sent again.
- The model sees each tool as `mcp__<server>__<tool>`, with the server's
  description (cut at 2048 characters) and input schema. A server name is 1 to
  32 letters, digits, `_`, or `-`, without `__` and not ending in `_`. A tool
  whose full name would exceed 64 characters or contain other characters, or
  whose input schema is larger than 64 KiB, is not offered, with a warning. A
  server offers at most 64 tools and 128 KiB of descriptions and schemas, and
  all servers together 100 tools and 256 KiB; the tools beyond that, in the
  order the server lists them, are left out with one warning per server.
- `url` is the server's MCP endpoint, used as written. It must use https;
  plain http is accepted only for localhost and loopback addresses. It may not
  contain credentials, a query, or a fragment. `headers` are sent with every
  request, with `${NAME}` expanded as in `env`; put tokens there, read from
  your environment rather than written in the file. Socium does not show header
  values: startup lists only the URL and the header names, and a value (or the
  token after `Bearer`) that the server writes back into an error is shown as
  `[redacted]`. An error in `config.toml` names its line, column, and key without
  quoting the line or a value from it. A proxy from the environment is used,
  except for a loopback URL. `env` does not
  apply to a `url` server, nor `headers` to a `command` server.
- A remote server receives every argument the model passes to its tools, and
  what it does with them is up to its operator, as with a model provider.
  Socium speaks the Streamable HTTP transport of MCP 2025-06-18 only: no
  older HTTP+SSE servers and no redirects. When the server ends the session
  (HTTP 404), Socium starts a new one; the call that got the 404 did not run
  and is not sent again, and the model is told to call the tool again. On exit
  Socium ends the session with a DELETE.
- A server that needs OAuth (MCP 2025-06-18 authorization) is logged in to
  once with `socium mcp login <server>` (see [usage](usage.md#mcp-login));
  until then it is skipped with the warning `MCP server <server> needs you to
  log in: run socium mcp login <server>`. Socium then sends the stored token
  as `Authorization: Bearer` on every request, renews it with its refresh
  token shortly before it expires or once after a 401, and shows the same
  warning when renewal fails. A configured `Authorization` header wins: with
  one, the stored token is not used.
- `oauth` is needed only for a client you registered with the server's
  authorization server ahead of time, such as a GitHub OAuth app (GitHub's
  server does not register clients itself). `client_id` is the client,
  `client_secret` its secret with `${NAME}` expanded as in `headers` (never
  shown or stored), `scopes` the scopes to ask for (by default those the
  server lists), and `callback_port` the fixed port of the redirect URI
  `http://127.0.0.1:<port>/callback` registered for that client. Without
  `oauth`, login registers a new client each time. `oauth` does not apply to
  a `command` server.
- Tokens are kept per user in
  `$XDG_STATE_HOME/socium/mcp-auth/<server>.json` (by default
  `~/.local/state/socium/mcp-auth/`), owner-only, never in the workspace;
  Socium refuses to start in a workspace that contains that directory. A
  token is used only while the server's `url` is the one it was issued for.
- A tool call asks for approval, since nothing confines what the server does
  (the request says whether it runs on the host or at the server's address);
  you can allow that one tool for the rest of the session. Tools listed in
  `allow` (their names on the server) run without asking. `socium -p` refuses a
  call that needs approval and tells the model to add the tool to `allow`.
- Servers start in parallel while the interface opens, and the first turn waits
  for them, at most 30 seconds each; you can cancel the wait. A server must
  answer with protocol revision 2025-06-18, 2025-03-26, or 2024-11-05. A server
  that fails to start is skipped with a warning; a local one's ends with its
  last standard error lines. Its standard error is never sent to the model, and
  `env` values of 8 or more characters are shown as `[redacted]` in warnings
  and errors. A server that writes a value into a tool result is not filtered.
- When Socium exits it closes each local server's standard input, waits a second,
  sends the server's process group SIGTERM, waits another second, and then
  sends SIGKILL.
- Subagents do not get MCP tools.

The design and what is deferred are in
[ADR 20](adr/0020-mcpサーバーを利用者の設定だけからstdioで起動しtoolを既定で尋ねて呼ぶ.md)
and, for remote servers, [ADR 21](adr/0021-遠隔のmcpサーバーにstreamable-httpとヘッダーだけで接続する.md)
and [ADR 22](adr/0022-遠隔のmcpサーバーをcliのoauthログインで認可しトークンを利用者の状態に保つ.md) (OAuth).

## Project instructions

Socium reads `AGENTS.md` (or `CLAUDE.md`) in the workspace root into its
instructions, again before every turn, so an edit applies from the next one.
A file in a subdirectory applies to the files under it: it is added to the
result of the first tool call that reads or edits a path there, and again
after it changes. Files outside the workspace (through a symlink) or over
32 KiB are not used, and Socium says so.

## Subagent types

Define your own subagent types in `~/.config/socium/agents/*.md` (or
`$XDG_CONFIG_HOME/socium/agents/`, or the directory named by
`SOCIUM_AGENTS_DIR`): a TOML front matter between `+++` lines with `name`,
`description`, and optionally `tools` (read-only tools when omitted) and
`model` (a registry alias; the current model when omitted), followed by the
type's instructions. Socium puts them after the framing every subagent gets
(it works in your workspace, cannot see the main conversation, and returns
only its final message), so write only what the role does. Start from
[`agent.md.example`](../agent.md.example). Definitions are read at startup;
a file with a mistake, or whose model cannot start, is skipped with a
warning. Definitions in the project directory are not read. See
[ADR 9](adr/0009-利用者定義のサブエージェントを設定ディレクトリのmarkdownで定める.md).
