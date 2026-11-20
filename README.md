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
- **You approve what runs.** Commands, commits, edits to files the host runs
  (build scripts, `Cargo.toml`, CI, editor settings), and reads of secret files
  ask first, showing the command, the diff, or the file. Other edits apply at
  once and can be undone with `/undo`. Allow once, allow for the session, or
  tell Socium what to do instead.
- **Your model, your session.** Claude by default, or any provider you add in
  `models.toml`. Stop at any time and resume with `-c` or `-r`.

<p align="center">
  <img src="docs/assets/demo.gif" width="860" alt="Socium runs a failing test, reads the file, fixes it, and runs the test again, asking before each command">
</p>

Socium is pre-release software developed by one person. It runs on Linux only
and expects breaking changes. The first tag is `v0.1.0`. It is tested on
Ubuntu 24.04 on x86_64, locally and in CI. Other distributions and aarch64
have not been tested, and macOS and Windows are not supported.

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

For a provider other than Anthropic, name its key's variable and the model in
`~/.config/socium/models.toml`, for example DeepSeek:

```toml
default = "ds-flash"

[providers.deepseek]
wire = "anthropic"
base_url = "https://api.deepseek.com/anthropic"
api_key_env = "DEEPSEEK_API_KEY"
auth = "bearer"

[[models]]
alias = "ds-flash"
provider = "deepseek"
id = "deepseek-flash"
context_window = 1000000
```

Then `export DEEPSEEK_API_KEY=...` and run `socium`. OpenAI, OpenRouter, Z.ai
and local servers are set up the same way; see Models below.

Ask for something in the composer and press Enter. Commands ask for your
approval before they run; `/help` lists commands and keys, and `/exit` quits.
To let builds see your toolchains and caches, copy
[`config.toml.example`](config.toml.example) to `~/.config/socium/config.toml`.

## Configure

- [Models](docs/configuration.md#models): Claude by default; register DeepSeek,
  Z.ai, OpenAI, OpenRouter, or a local server in `~/.config/socium/models.toml`.
- [Sandbox and approvals](docs/configuration.md#sandbox-and-approval-settings):
  which host directories commands may read, and which commands run without
  asking.
- [Environment variables](docs/configuration.md#environment-variables):
  `MODEL`, `EFFORT`, `MAX_TOKENS`, and the rest.

## Documentation

- [Using Socium](docs/usage.md): approvals, the Activity pane, subagents,
  sessions, `/rewind`, and `socium -p` for scripts.
- [Configuration](docs/configuration.md): requirements, models, sandbox, web
  search, and `AGENTS.md`.
- [Linux sandbox](docs/linux-sandbox.md): the exact filesystem policy and
  networked builds.
- [Reliability and recovery](docs/daily-reliability.md): storage limits,
  editing guarantees, and recovery.
- [All documentation](docs/index.md).

## Security

Only run Socium in workspaces you trust; see the
[threat model](docs/threat-model.md). Read files and command output can be
sent to the model provider; `.gitignore` is not a confidentiality boundary.
Conversation, edits, and command output are stored under `.socium/` without
encryption, so add `.socium/` to your `.gitignore` and keep API keys outside
source files. Report vulnerabilities as described in [SECURITY.md](SECURITY.md).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for running from a checkout and the
checks CI runs. Licensed under [MIT](LICENSE). The Nix package carries the
license texts of the crates it is built from in `share/doc/socium`.
