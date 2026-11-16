<p align="center">
  <img src="docs/assets/logo.svg" width="72" height="72" alt="">
</p>

<h1 align="center">socium</h1>

<p align="center">
  A Rust coding agent for the terminal that runs every command in a Linux
  sandbox with no network and no access to your home directory.
</p>

<p align="center">
  <a href="https://github.com/chansuke/socium/actions/workflows/ci.yaml"><img src="https://github.com/chansuke/socium/actions/workflows/ci.yaml/badge.svg?branch=main" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="License: MIT"></a>
  <img src="https://img.shields.io/badge/platform-Linux-lightgrey" alt="Platform: Linux">
</p>

- **Sandboxed by default.** Every command runs under bubblewrap and seccomp.
  Socium refuses to start if isolation fails; there is no unsandboxed fallback.
- **You approve what runs.** Commands and edits ask first, with the full diff
  to review. Allow once, allow for the session, or tell Socium what to do
  instead.
- **Your model, your session.** Claude by default, or any provider you add in
  `models.toml`. Stop at any time and resume with `-c` or `-r`.

Socium is pre-release software developed by one person. It runs on Linux only
and expects breaking changes. The first tag is `v0.1.0`.

## Install

With Nix, run it straight from GitHub, or install it into your profile:

```bash
nix run github:chansuke/socium
nix profile install github:chansuke/socium
```

The Nix package brings Git, ripgrep, and bubblewrap 0.12 with it. Without Nix,
build with Cargo (Rust 1.92 or newer, a C compiler, and CMake for the TLS
library), and install Git, ripgrep, and bubblewrap 0.12 or newer yourself:

```bash
cargo install --locked --git https://github.com/chansuke/socium socium-tui
```

The installed command is `socium`.

On Ubuntu 24.04 and later, bubblewrap also needs an AppArmor `userns` profile;
see the [Linux sandbox guide](docs/linux-sandbox.md#installation-and-support).

## Quick start

```bash
export ANTHROPIC_API_KEY=...   # or register another provider; see Models below
cd your-project
socium
```

Ask for something in the composer and press Enter. Commands ask for your
approval before they run; `/help` lists commands and keys, and `/exit` quits.
To let builds see your toolchains and caches, copy
[`config.toml.example`](config.toml.example) to `~/.config/socium/config.toml`.

## Workspace layout

- `crates/core`: domain logic, providers, tool runtime, and input handling.
- `crates/tui`: Ratatui application entrypoint (binary crate).
- `evals/`: fixed tasks for comparing changes against a real model.

## Run from a checkout

```bash
nix develop -c cargo run --locked -p socium-tui
```

`nix develop` provides the pinned toolchain and bubblewrap 0.12.0. To build and
run the packaged application, use `nix run .`. Plain `cargo run` outside the
development shell uses the host `bwrap`, and distribution packages older than
0.12.0 (for example Ubuntu's 0.9.0) are refused at startup. On Ubuntu, the
Nix-provided `bwrap` also needs an AppArmor `userns` profile; see the
[Linux sandbox guide](docs/linux-sandbox.md). Execution currently requires Linux
(x86_64 or aarch64), bubblewrap, and unprivileged user namespaces. The TUI checks
the sandbox at startup and refuses to run if isolation cannot be established.
macOS and Windows execution are unsupported; there is no unsandboxed fallback.

Set `ANTHROPIC_API_KEY` before starting. The default model is Claude Opus 5.5
(`claude-opus-5-5`); set `MODEL` to a model alias or ID to choose another at
startup. Thinking is adaptive on Claude models. `THINKING_DISPLAY` chooses what
the API returns as text: `summarized` (default; reasoning summaries and progress
updates), `updates` (progress updates between tool calls only), or `omitted`.
`EFFORT` (`low`, `medium`, `high`, `xhigh`, `max`) sets thinking depth; when
unset the model's default applies. `THINKING_BUDGET_TOKENS` and `COMPACT_MODEL`
are no longer supported. `MAX_TOKENS` caps each response, thinking included, and
defaults to 64000; a model's `max_output_tokens` in `models.toml` wins over it.
`CONTEXT_WINDOW_TOKENS` replaces every model's context window,
`COMPACTION_TOKEN_THRESHOLD` replaces the prompt size at which history is
compacted, and `ANTHROPIC_VERSION` sets the `anthropic-version` header
(default `2023-06-01`). Model names must be available to your API account.

`socium -p "<prompt>"` runs one turn without the terminal UI and prints its
progress and result as JSON Lines on standard output (`-p -` reads the prompt
from standard input). Each line has the schema version in `v` and a `type`:
`started`, then one `activity` per Activity event, then `finished` with `ok`,
the reply `text` or the `error`, and the tokens used. Diagnostics go to standard
error. Nobody can answer an approval there, so a call that needs one is refused
and the model is told how to allow it; Ctrl+C cancels. The exit status is 0, 1
on failure, 2 for a usage error, or 130 when cancelled. `-c` and `-r <ID>`
continue a saved conversation.

### Models

`/model` opens a numbered list of the registered models in place of the
composer, with a check on the current one; choose with the arrow keys and Enter,
or press its number. `/model <alias>` switches at once, and Tab completes the
alias. Switching is allowed between turns only and keeps the history whole:
each reply records the provider and model that wrote it, and thinking is sent
back only to a provider that can read it (Anthropic's API reads any Claude
model's thinking; other endpoints only their own model's). The choice is saved
with the conversation, so resuming it, or
starting a new conversation after it, uses the same model. At startup the saved
choice (of the resumed conversation, or of the most recently saved one for a new
conversation) wins over `MODEL`, which wins over
the registry default; when none of them has its API key set, Socium starts on
the first registered model that does and says so.

The built-in registry has `opus`, `sonnet`, and `haiku` on Anthropic, each with
a 1M-token context window. Add other
endpoints in `~/.config/socium/models.toml` (or `$XDG_CONFIG_HOME/socium/models.toml`, or the
file named by `SOCIUM_MODELS_FILE`); start from
[`models.toml.example`](models.toml.example). The file names the environment
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

`base_url` must use HTTPS, except on localhost. Anthropic extensions (adaptive
thinking, effort, eager tool input streaming, the built-in text editor tool
type, and prompt caching) are sent only to models whose `features` list them;
built-in Claude models list all five (`adaptive_thinking`, `effort`,
`eager_input_streaming`, `anthropic_text_editor`, `prompt_caching`), and other
models get none, receiving the text editor as an ordinary JSON-schema tool.
`compact_at_tokens` sets the prompt size at which a model's history is
compacted (default 200000, or 80% of the input budget if that is lower), and
`price` gives its US dollar prices per million tokens so the status line can
show the session's cost; Socium ships no prices.

Providers with `wire = "openai-chat"` speak OpenAI Chat Completions: OpenAI
itself (`https://api.openai.com/v1`), Ollama Cloud (`https://ollama.com/v1`),
OpenRouter, or a local server such as Ollama (`http://localhost:11434/v1`).
They use Bearer authentication, and only the `effort` feature applies, sent as
`reasoning_effort`. Reasoning text that some servers stream is shown in the
Activity pane but not kept in the history.

Providers with `wire = "openai-responses"` speak the OpenAI Responses API
(`https://api.openai.com/v1`), which lets reasoning models such as GPT-6 Astra
call tools; Chat Completions rejects tools with reasoning on that model. The
`effort` feature marks a reasoning model: its reasoning summaries are shown in
the Activity pane, `EFFORT` is sent as `reasoning.effort`, and its encrypted
reasoning is sent back with the next request so it carries across tool calls.
Nothing is stored on the server (`store: false`).

Model IDs above were checked against the DeepSeek and Z.ai documentation on 2026-11-13; DeepSeek rejects unknown
non-Claude names with 400. To check a registered model against its real API
(this costs money), run
`SOCIUM_LIVE_MODEL=<alias> nix develop -c cargo test -p socium-core live_ -- --ignored`.
Web search is not implemented; see
[SOC-037](docs/tickets/SOC-037-deepseek-web-search.md).

Install Git, ripgrep (`rg`), and bubblewrap (`bwrap`, version 0.12 or newer).
The Linux Nix package includes them on its runtime PATH. A project directory can
be an ordinary checkout or a linked worktree made by `git worktree add`; for a
worktree, the main repository's Git directory is mounted read-only (writable only
during `git_commit`). Submodule gitfiles and other redirected Git metadata are
refused.

### Sandbox and approval settings

`~/.config/socium/config.toml` (or `$XDG_CONFIG_HOME/socium/config.toml`, or the
file named by `SOCIUM_CONFIG_FILE`) says what commands may see beyond the
workspace and when a tool call needs your approval; start from
[`config.toml.example`](config.toml.example). The file must lie outside every
workspace Socium opens, so the model can never change it, and startup lists
what it adds.

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
- Edits to files a host tool runs or loads by itself (`build.rs`, `.envrc`,
  `flake.nix`, `.cargo/config.toml`, `.vscode/`, `.github/workflows/`, Git hook
  managers, and similar) always ask, and so do reads of files that commonly
  hold credentials (`.env`, `.netrc`, `*.pem`, SSH keys, and similar); `protected`
  and `sensitive` replace these lists. Approval has no time limit. A denied call
  tells the model not to retry it.

## Everyday work

- Read and search workspace files, preview patches, and undo recorded edits.
  Reads number every line and return 2,000 lines at a time, saying where to
  continue; long lines are cut. Each edit result, and the conversation, shows
  the edit's diff. `/undo` reverts Socium's last edit, and `/undo <path>` its
  last edit of that file; the history keeps the last 200 edits, up to 16 MiB.
  Command output beyond 20,000 characters keeps its start and, mostly, its end,
  where failures are reported; each stream is saved under `.socium/outputs/`
  (up to 8 MiB).
  Patches keep each untouched line's LF or CRLF ending and refuse files with a
  stray carriage return; moves and undo keep Unix permission bits such as the
  executable bit (not owners, ACLs, or extended attributes), and undo refuses
  a file whose mode was changed since; FIFOs and other special files are
  refused rather than read. Searches
  that stop at their match limit say the result is incomplete.
- Review commands, commit diffs, and protected edits in the TUI: choose with
  the arrow keys and Enter, or press a choice's number, `y` to allow once, `a`
  to allow for the session (when offered), or `n` to deny. While a request
  waits, the whole request, diff included,
  replaces the conversation: PgUp/PgDn (or Space/b) page through it and
  Home/End jump to its ends; these keys never answer. Commands can write
  workspace files, but cannot access the host home or network. Existing Git
  metadata is read-only except during `git_commit`, which commits a symlink
  named in `paths` as a link, never the file it points to; Socium's state is
  hidden from all child processes.
- Type `/` to show command suggestions: Up/Down select, Tab completes, Enter
  runs, Esc closes. `/commands` lists every command; `/help` adds key
  bindings. Type `exit` or `/exit` to quit.
- The Activity pane shows the model's reasoning summaries and progress
  updates, tool calls as their input is written, tool results and durations,
  and per-round stop reason, token use, and time to first token. Press Ctrl+A
  to focus it: Up/Down select, Ctrl+L jumps to the top, Enter expands, `r`
  shows the round's raw provider events, `z` fills the screen, Esc returns to
  the input. Recent activity is
  shown again after a restart. On terminals 140 columns or wider it sits beside
  the conversation. Its title shows what the agent is doing (investigating,
  editing, verifying), and it points out repeated calls, a change of course
  after a failure, and long stretches of tool rounds without an update.
- The model can hand a self-contained task to a subagent with the `agent`
  tool. The subagent starts from an empty context and returns only its final
  report, so a broad search stays out of the conversation. An `explorer` can
  only read, list, and search files and inspect Git; a `worker` can also edit
  and run commands, which still need your approval; the approval names the
  subagent that asked. The report ends with the subagent's rounds, tool
  calls, token use, time, and its ID. A subagent runs on the current model unless
  its type or the call names another registered model whose API key is set,
  so a simple search can go to a smaller model. Subagents cannot start
  subagents, and the main agent waits for them. New subagents of types that
  cannot change the workspace (such as `explorer`), called one after another
  in the same response, run at the same time, tool calls included; others
  run one at a time. Each subagent's steps appear in the Activity pane under
  its own `agent` call, marked with `│`, and its repeated calls and silent
  rounds are counted apart from the other agents'. Each
  subagent's conversation is saved in `.socium/agents/<id>.json` before each
  of its tool calls; the model can continue a subagent with everything it
  saw by passing its ID as `resume`, and `/agents` lists saved subagents and
  `/agents <id>` shows one's conversation. The newest 100 are kept. A restart
  during a subagent still marks the `agent` call's outcome unknown; the
  reports of subagents running with it that had already finished are kept. See
  [ADR 8](docs/adr/0008-agentツールで独立した文脈のサブエージェントに作業を委ねる.md).
- Define your own subagent types in `~/.config/socium/agents/*.md` (or
  `$XDG_CONFIG_HOME/socium/agents/`, or the directory named by
  `SOCIUM_AGENTS_DIR`): a TOML front matter between `+++` lines with `name`,
  `description`, and optionally `tools` (read-only tools when omitted) and
  `model` (a registry alias; the current model when omitted), followed by the
  subagent's system prompt. Start from
  [`agent.md.example`](agent.md.example). Definitions are read at startup;
  a file with a mistake, or whose model cannot start, is skipped with a
  warning. Definitions in the project directory are not read. See
  [ADR 9](docs/adr/0009-利用者定義のサブエージェントを設定ディレクトリのmarkdownで定める.md).
- The composer takes pasted text whole, newlines included. Alt+Enter or
  Shift+Enter (where the terminal reports it), or `\` before Enter, starts a
  new line; Up/Down move between the lines of a message. You can write the next
  message while a request runs and send it once the request ends.
- The status line shows how much of the input budget is left, the tokens the
  conversation has used and the share read from the prompt cache, and the cost
  when every model used has a `price`.
- When the provider is overloaded, rate limits, or the connection fails or goes
  quiet for 180 seconds before any output, the request is retried up to three
  times; a long answer is never cut off while data keeps arriving. An answer
  that stops at the output limit is kept with a notice.
- Press Esc or Ctrl+C during a task to cancel and wait for cleanup. In normal
  mode (after Esc) `q` no longer quits; use `/exit` or Ctrl+C.
- Each start opens a new conversation. `-c` (`--continue`) continues the most
  recently saved one in the workspace, and `-r` (`--resume`) lists the saved
  ones, newest first, to choose from; `-r <ID>` opens one directly, by its full
  ID, its name, or any unique ID prefix. `/resume` opens the same list between
  turns (`/resume <ID>` switches at once); in the list, `d` then `y`
  permanently deletes the selected conversation. `/rename <name>` names the
  open conversation. On exit Socium prints the command that resumes the
  conversation. `/clear` starts a new conversation and keeps the previous one
  resumable. Resuming loads the conversation and Activity, and the
  conversation shows each tool call with its argument, a one-line result,
  whether it was approved, and failed requests; interrupted operations with no
  saved result are marked unknown, never replayed. Only one Socium runs in a
  workspace at a time.
- `/rewind` lists your prompts in this conversation, newest first. Choosing one
  drops it and everything after it and puts it back in the input to edit and
  send again. When Socium edited files from that prompt on, you choose whether
  to restore the conversation, the files, or both. Files are restored only if
  they still hold what Socium wrote; otherwise nothing changes. Edits whose
  turns were dropped by a conversation-only rewind stay for `/undo`.
- `/compact`, automatic compaction, and budget trimming replace older turns
  with a summary. Thinking in the turns kept verbatim was produced against the
  replaced history, so it is no longer sent back; thinking produced after the
  summary is. The full history stays in `.socium/` either way, and summary
  updates keep room for the newest messages. The first request is kept word
  for word beside the summary. One tool result holds at most 100,000
  characters; when the results of a round would overflow the window, they are
  cut to fit, and if that is not enough the turn stops, the history is
  compacted, and you can ask Socium to continue.
- Conversation, Activity, and code blocks wrap by terminal cells, so CJK text,
  combining characters, and emoji sequences are not cut off.

Add `.socium/` to your project's `.gitignore`. See the
[Linux sandbox guide](docs/linux-sandbox.md) for the exact filesystem policy,
environment, toolchain requirements, and restrictions on networked builds.
See the
[reliability and recovery guide](docs/daily-reliability.md) for storage limits,
editing guarantees, migration, and recovery procedures.

Only run Socium in workspaces you trust; see the
[threat model](docs/threat-model.md). Read files and command output can be
sent to the model provider; `.gitignore` is not a confidentiality boundary.
Socium reads `AGENTS.md` (or `CLAUDE.md`) in the workspace root into its
instructions.
Conversation, input history, edits, and command artifacts are stored locally
under `.socium/` without encryption. Keep API keys outside source files.

See [documentation](docs/index.md), [contributing](CONTRIBUTING.md), and the
[security policy](SECURITY.md). Licensed under [MIT](LICENSE).
