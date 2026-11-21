# Architecture

Socium has two crates. `socium-core` holds the agent: conversation, providers,
tools, sandbox, approvals, and saved state. `socium-tui` is the binary. It
draws the terminal UI and runs `socium -p`, and it reaches the core only
through what `socium-core` makes public.

## Layers of socium-core

The top-level modules of `socium-core` are in layers. A module may use only
modules in lower layers, never one in its own layer or above.
`crates/core/tests/architecture.rs` checks this on every `cargo test`, so
the module graph has no cycles. `lib.rs` is the facade above every layer.
Test code is not checked.

| Layer | Modules | What they hold |
|---|---|---|
| 0 | `constants`, `host`, `tool_error` | Fixed values, the process environment and host paths, the tool error type |
| 1 | `conversation`, `models` | Messages, content blocks, tool definitions and requests; the model registry and prices |
| 2 | `state`, `providers` | Files under `.socium/`, written atomically and under a lock; the wire formats and streams of each provider |
| 3 | `plan`, `settings`, `error` | The agent's plan; `config.toml`; the agent error |
| 4 | `activity`, `execution`, `workspace` | Activity events and signals; the Linux sandbox; resolving paths inside the workspace |
| 5 | `transcript`, `usage`, `access`, `activity_log` | The transcript after a restart, token use and cost, session grants, withheld files and the quarantine of protected files a command left, the saved activity log |
| 6 | `progress` | `AgentProgress`, what a running turn reports to the UI |
| 7 | `control` | `TaskControl`: cancellation, approvals waiting for an answer, steering |
| 8 | `process` | Running a command in the sandbox with bounded output |
| 9 | `tools` | The built-in tools and the tool list a model is offered |
| 10 | `approval`, `instructions` | Whether a call runs, asks, or is refused; nested instruction files, which tools read |
| 11 | `dispatch`, `session` | Running one tool call through the gate; saved sessions |
| 12 | `agent`, `model_import` | Turns, context, compaction, and subagents; `models.d` files written from the models.dev catalog |

When a lower module needs something from a higher one, change the design
instead of the check. Move what both need down into a lower module, or let
the caller pass it in as a value, a function, or a trait the lower module
defines. For example, `tools::tools_with_settings` takes the `agent` tool
definition from its caller instead of reading the subagent catalog, and
`session::migrate_legacy_state` takes the step that moves subagent records.
A new top-level module goes in the lowest layer that holds everything it
uses.

## APIs with a single entry point

Clippy (`clippy.toml`) refuses these APIs outside the one place that wraps
them. Each reason in `clippy.toml` names what to use instead.

- Waiting: wait for the event itself, using `Condvar` or `tokio::sync::Notify`
  in `TaskControl`, `poll(2)` on a descriptor, or a channel. A deliberate
  delay such as a retry backoff uses `TaskControl::pause`, which ends early
  on cancel.
- The environment: settings come through the `EnvLookup` that `AgentService`
  is built with, so tests can inject values. `host` reads `HOME`, `PATH`, and
  `XDG_CONFIG_HOME`.
- HTTP clients: `http_client` builds every provider client with no redirects
  and with connect and idle timeouts. Tests use `test_support::http_client`.

An exception takes `#[expect(clippy::disallowed_methods, reason = "...")]`
on the smallest item, with a reason that says why the wrapper cannot apply.
