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

Gemini models use the Gemini API with a key from Google AI Studio
(https://aistudio.google.com/apikey). `base_url` includes the API version:

```toml
[providers.gemini]
wire = "google"
base_url = "https://generativelanguage.googleapis.com/v1beta"
api_key_env = "GEMINI_API_KEY"

[[models]]
alias = "gemini"
provider = "gemini"
id = "gemini-4-carbon"
description = "Gemini 4 Carbon for complex coding"
features = ["effort"]
context_window = 1048576
max_output_tokens = 65536
```

The model ID and limits were checked against https://aistudio.google.com/docs/models
on 2026-11-20. OpenCode Zen serves Gemini models on the same wire at
`https://opencode.ai/inference/google/v1beta`; set `auth = "bearer"` for it.

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
- `wire = "google"` speaks Google's Gemini API (`streamGenerateContent`).
  The key goes in the `x-goog-api-key` header by default; `auth = "bearer"`
  sends it as a Bearer token instead. Only the `effort` feature applies. It
  marks a thinking model: its thought summaries are shown in the Activity
  pane, and `EFFORT` is sent as `thinkingLevel` (`max` becomes `xhigh`).
  Gemini returns thought signatures that it needs back on later requests,
  so they are kept in the history and sent back to the same model.

To check a registered model against its real API (this costs money), run
`SOCIUM_LIVE_MODEL=<alias> nix develop -c cargo test -p socium-core live_ -- --ignored`.

### Importing from models.dev

`socium models import <provider>...` registers the models of providers listed
on [models.dev](https://models.dev), the catalog OpenCode uses. It downloads
`https://models.dev/api.json` (or reads a saved copy with `--from FILE`) and
writes `models.d/<provider>.toml` next to `models.toml`. Running it again
overwrites the file. Without a provider, it lists the providers and how many
of their models it can import.

Socium reads the built-in models first, then every `*.toml` in `models.d/` in
file-name order, then `models.toml`. A later entry with the same alias
replaces an earlier one, so put your own changes in `models.toml`. Only
`models.toml` may set `default`, and two files in `models.d/` may not define
the same provider.

Aliases look like `opencode/claude-haiku-5-5`. A provider whose models use
several API formats, such as OpenCode Zen, becomes one provider per wire
(`opencode`, `opencode-anthropic`, `opencode-openai-responses`, and so on),
all with the same key. Deprecated models, models that do not answer in text,
and providers that need OAuth, cloud credentials, or an unsupported SDK are
skipped, and the command says why.

The catalog is read only when you run the command, never at startup. Your API
key is sent to each provider's `base_url`, so read the base URLs the command
prints, and the file it writes, before you set the key.

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
- No setting turns every approval off. The `--dangerously-skip-permissions`
  flag does that for one run: nothing asks and commands get no narrower
  sandbox, but `deny` prefixes are still refused and the sandbox is unchanged.

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

## Tool profiles

`[tools] profile` in `config.toml` picks which built-in tools the model is
offered, so you can compare tool sets with the eval harness. `full` (default)
offers all of them. `basic` offers `read_file`, `apply_patch`, the text editor,
`run_command`, and `git_commit`, and `search` adds `search_text` and
`find_symbol`. `agent`, `update_plan`, and `web_search` do not depend on the
profile. Subagents get only the tools of their type that the profile offers,
and a call to a tool outside the profile is refused without running. Startup
names the profile when it is not `full`.

`find_symbol` is a quick search for Rust declarations, not a symbol index. It
reads only `.rs` files and matches lines that start a declaration (`fn`,
`struct`, `enum`, `trait`, `type`, `mod`, `const`, `static`, or an `impl`
naming the type), so it also returns such lines in comments and strings,
misses a declaration split across lines, and cannot tell same-named items
apart or find references. For other languages the model uses `search_text`.
[ADR 21](adr/0021-find-symbolはrustの宣言行の検索に留めtree-sitterとlspは入れない.md)
records why there is no parser-based backend.

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
