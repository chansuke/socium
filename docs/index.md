# Documentation

Start with the [README](../README.md) for installation, then [Using Socium](usage.md).

## Current behavior

- [Using Socium](usage.md): the terminal UI, approvals, subagents, sessions, and `socium -p`.
- [Configuration](configuration.md): requirements, environment variables, models, sandbox and approval settings, and MCP servers.
- [Reliability and recovery](daily-reliability.md): permissions, editing, cancellation, saved state, and recovery.
- [Linux sandbox](linux-sandbox.md): enforced execution policy, installation, limits, and tests.
- [Threat model](threat-model.md): what is trusted, what is protected and how, and what is not.
- [Security policy](../SECURITY.md): trust boundaries and private vulnerability reporting.
- [Contributing](../CONTRIBUTING.md): development and verification.
- [Verification checklist](verification.md): checks a diff triggers, each from a bug fixed here before.
- [Benchmark](../bench/README.md): what a run costs apart from the model, compared with the parent commit in CI.
- [Evaluation](../evals/README.md): fixed tasks with hidden checks, run against a real model within a budget.

## Design decisions

- [Architecture decision records](adr/README.md): decisions and the options they were weighed against.
- [Activity stream design](design-agent-activity-stream.md): showing model reasoning and agent execution in the Activity pane.

## Results

The [2026-11-19 benchmark results](benchmark-2026-11-19.md) compare Socium
with seven other agents and explain why the lean-ctx proxy is not used.

Planned work and its state are tracked in [GitHub Issues](https://github.com/chansuke/socium/issues).
