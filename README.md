# socium

A Rust coding agent for the terminal.

Our goal is a Rust coding agent that is easy to install and customize, works with
your preferred model, preserves your existing changes, and lets you stop and
resume work.

## Workspace layout

- `crates/core`: domain logic, providers, tool runtime, and input handling.
- `crates/tui`: Ratatui application entrypoint (binary crate).

## Run

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
defaults to 64000 unless the model sets `max_output_tokens`. Model names must be
available to your API account.

### Models

`/model` opens a numbered list of the registered models in place of the
composer, with a check on the current one; choose with the arrow keys and Enter,
or press its number. `/model <alias>` switches at once, and Tab completes the
alias. Switching is allowed between turns only, removes earlier
thinking blocks from the history (their signatures only work with the model
that produced them), and is saved with the conversation, so a restart resumes
the same model. At startup the saved choice wins over `MODEL`, which wins over
the registry default; when none of them has its API key set, Socium starts on
the first registered model that does and says so.

The built-in registry has `opus`, `sonnet`, and `haiku` on Anthropic. Add other
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
thinking, effort, eager tool input streaming, and the built-in text editor tool
type) are sent only to models whose `features` list them; built-in Claude
models list all four (`adaptive_thinking`, `effort`, `eager_input_streaming`,
`anthropic_text_editor`), and other models get none, receiving the text editor
as an ordinary JSON-schema tool.

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
Web search is planned; see
[ADR 6](docs/adr/0006-モデルを接続形式と接続先の組で登録しmodelコマンドで切り替える.md).

Install Git, ripgrep (`rg`), and bubblewrap (`bwrap`, version 0.12 or newer).
The Linux Nix package includes them on its runtime PATH. Use a project directory
with an ordinary `.git` directory; linked worktrees and redirected Git metadata
are not yet supported.

## Everyday work

- Read and search workspace files, preview patches, and undo recorded edits.
- Review sandboxed commands and commit diffs in the TUI; press `y` to allow once
  or `n` to deny. Commands can write workspace files, but cannot access the host
  home or network. Existing Git metadata is read-only except during `git_commit`;
  Socium's state is hidden from all child processes.
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
- Press Esc or Ctrl+C during a task to cancel and wait for cleanup.
- Restart in the same workspace to load saved conversation and tool results.
  Interrupted operations with no saved result are marked unknown, never replayed.

Add `.socium/` to your project's `.gitignore`. See the
[Linux sandbox guide](docs/linux-sandbox.md) for the exact filesystem policy,
environment, toolchain requirements, and restrictions on networked builds.
See the
[reliability and recovery guide](docs/daily-reliability.md) for storage limits,
editing guarantees, migration, and recovery procedures.

Only run Socium in workspaces you trust. Read files and command output can be
sent to the model provider; `.gitignore` is not a confidentiality boundary.
Conversation, input history, edits, and command artifacts are stored locally
under `.socium/` without encryption. Keep API keys outside source files.

See [documentation](docs/index.md), [contributing](CONTRIBUTING.md), and the
[security policy](SECURITY.md). Licensed under [MIT](LICENSE).
