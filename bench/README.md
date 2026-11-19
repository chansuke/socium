# Benchmark

Measures what Socium costs to run apart from the model, so that a change
which makes every request larger or every run slower shows up in the pull
request that makes it. The [evaluation tasks](../evals/README.md) measure
whether the agent does the work; this measures what the work costs.

Each scenario in `scenarios.toml` is a fixed conversation. A loopback model,
`mock_model.py`, answers the n-th request with the n-th tool call of the
scenario and then with a short reply, so every run does the same work and no
run calls a paid model.

## Running

Build each side, then run them in turn within each repetition:

```sh
nix build .#default --out-link head
nix build "git+file://$PWD?rev=$(git rev-parse main)#default" --out-link base
nix develop -c python3 bench/run.py \
  --binary base=base/bin/socium --binary head=head/bin/socium \
  --repeat 7 --out bench.json
python3 bench/compare.py bench.json
```

`--scenario NAME` runs only the named scenarios. `--lean-ctx LABEL=PATH`
sends one side's requests through the [lean-ctx](https://github.com/yvgude/lean-ctx)
proxy in its cache mode. Give the same binary two labels to compare the two
routes.

A run counts only when Socium exits 0, the model served every call of the
scenario plus the final reply, the tool calls failed as often as the
scenario's `tool_errors` says, and the last request holds the scenario's
`expect` texts. Only the real tool output contains those texts, so a tool that
returned an error or nothing fails the run. Failed runs are left out of the
numbers and listed under the table.

## Results

| Field | Meaning |
|---|---|
| `requests` | requests the model received |
| `request_bytes` | bytes of all request bodies, which is mostly the history sent again each round |
| `first_request_bytes` | the system prompt, tool definitions, and prompt |
| `tool_result_bytes` | tool output in the last request |
| `wall_ms` | from start to exit of `socium -p` |
| `cpu_ms`, `max_rss_kib` | CPU time and peak memory of Socium and the sandboxed commands it waited for |
| `startup_ms` | `socium --help`, five times per repetition |
| `size_bytes` | the executable, not the Nix wrapper script |
| `closure_bytes` | the Nix store path and everything it needs, as `nix path-info -S` reports |

The request fields do not change between runs of the same binary.
`compare.py` shows any change in them. The time and memory fields vary, so
`compare.py` calls head better or worse only when the ranges of the two sides
do not overlap. Otherwise it shows `~`.

## In CI

The [Benchmark workflow](../.github/workflows/bench.yaml) builds the parent
commit and the checked-out commit as Nix packages, runs both with
`--repeat 7`, and writes the table to the job summary. A pull request checks
out its merge commit, so the parent is the base branch. The job fails only
when a run fails. A change in size or time does not fail it.

GitHub's runners are shared machines, so small time differences show as `~`.
The request fields have no noise, and any change in them is real.

## Other agents

`agents.py` asks Socium and other coding agents from
[llm-agents.nix](https://github.com/numtide/llm-agents.nix) "Say hello." once
each, in their headless modes, against the same loopback model on each
agent's own wire:

```sh
nix build github:numtide/llm-agents.nix#claude-code --out-link claude-code
nix develop -c python3 bench/agents.py socium=head/bin/socium \
  claude-code=claude-code/bin/claude --repeat 7 --out agents.json
```

It knows `socium`, `claude-code`, `codex`, `opencode`, `crush`, `pi`,
`gemini-cli`, and `goose`. It records the requests and bytes each agent sends,
the time and peak memory of the run, `--version` time, and the size of the Nix
closure. The agents do different work for the same prompt, such as a title
request or a routing request, and the request paths in the output show which.
The [2026-11-19 results](../docs/benchmark-2026-11-19.md) were made this way.
