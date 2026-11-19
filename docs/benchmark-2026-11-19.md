# Benchmark results, 2026-11-19

These results compare Socium with seven coding agents from
[llm-agents.nix](https://github.com/numtide/llm-agents.nix) at commit
`7a60511`, and test whether the [lean-ctx](https://github.com/yvgude/lean-ctx)
proxy makes Socium faster. They describe Socium at `aaab51a`. The raw numbers
are in [review-evidence/2026-11-19](review-evidence/2026-11-19/), and
[bench/README.md](../bench/README.md) explains how to rerun them.

All runs used one 24-core Linux machine with a load average near 2, with 7
runs per agent, interleaved. Every agent talked to the same loopback model,
so no time below includes a real model.

## Socium starts and answers fastest

Each agent was asked "Say hello." once in its headless mode
(`bench/agents.py`). Wall and CPU time cover the whole process tree.

| Agent | Wall ms, median (range) | CPU ms | Peak MiB | `--version` ms | Requests | Bytes sent | Closure MiB |
|---|---|---|---|---|---|---|---|
| socium 0.1.0 | 29 (27–31) | 15 | 18 | 4 | 1 | 13,933 | 358 |
| crush 0.101.0 | 95 (82–103) | 217 | 89 | 29 | 2 | 52,119 | 134 |
| claude-code 2.1.327 | 303 (290–359) | 374 | 203 | 10 | 1 | 47,874 | 422 |
| pi 1.5.2 | 319 (302–341) | 426 | 92 | 239 | 1 | 5,410 | 189 |
| goose 1.55.0 | 349 (209–404) | 434 | 223 | 10 | 2 | 18,785 | 379 |
| codex 0.170.0 | 436 (407–464) | 282 | 168 | 14 | 1 | 39,873 | 690 |
| opencode 1.19.6 | 1895 (1711–2020) | 3545 | 656 | 330 | 2 | 33,230 | 276 |
| gemini-cli 0.68.0 | 2460 (2415–2512) | 3138 | 310 | 2294 | 2 | 45,955 | 722 |

Socium's `--version` column is `--help`, because Socium has no `--version`.
The second request of crush, goose, and opencode is a title or summary
request. The second request of gemini-cli is a model-routing request. One of
the seven crush runs printed no reply, so its row has six runs.

Socium sends the second-smallest first request. Of its 13,933 bytes, 12,010
are tool definitions and 2,205 are the system prompt. pi sends 5,410 bytes.

## The closure is now 165 MiB

Of Socium's 358 MiB closure, 341 MiB came from `git`, which brings Perl and
Python. Socium and the commands it runs need neither, so the package now uses
`gitMinimal`. The closure is 165 MiB, so a first `nix run` has 193 MiB less
to install. The 642 package tests pass with `gitMinimal`, and the benchmark sends
the same bytes with the same times. The commands that `gitMinimal` lacks are
`git send-email`, `git svn`, `git p4`, and the manual pages.

## lean-ctx is not used

lean-ctx 3.12.6 can sit between an agent and the model API as a proxy that
compresses requests. Socium needs no code change for that. Socium only needs
`base_url` in `models.toml` to point at the proxy. The proxy was tested with
`bench/run.py --lean-ctx`, in front of the same loopback model. Socium does
not use it, for three reasons.

The default `token` mode rewrites the system prompt. It cut Socium's 2,205
bytes to 1,053 and dropped the planning, sandbox, and approval paragraphs,
including "If the user denies a call, do not retry it". The mode also added
"Be concise" to the user's message.

The `cache` mode keeps the system prompt and the user's words. It still
removes content that the model asked for. After `read_file` of the 1,161-line
`legacy_billing.py`, the model received the first 4 and the last 10 lines,
"[1147 lines omitted]", and a pointer to a lean-ctx tool that Socium does not
have. The next request held the whole file again. In long command output, the
proxy removed Socium's own note that the full output is in `.socium/outputs`.
It pointed instead to a file under the proxy's home directory, which the
sandbox hides from commands.

The proxy also made every run slower:

| Scenario | Bytes sent, direct → lean-ctx | Wall ms, direct → lean-ctx |
|---|---|---|
| hello | 13,933 → 13,933 | 29 → 216 |
| read-and-search | 200,161 → 158,445 (−20.8%) | 245 → 568 |
| long-command-output | 84,388 → 77,144 (−8.6%) | 142 → 515 |

The byte savings in `read-and-search` come entirely from the shortened file.
A real model takes seconds for each request, so the proxy's time cost
matters less there. The removed file content and system prompt do not.

## With a real model, the model takes the time

`ds-flash` (DeepSeek-V4.1-Flash) ran all 39 evaluation tasks 3 times with
Socium at `8cf152b`. All 117 runs passed, for $0.88 at DeepSeek's peak prices.
The median run took 16.4 seconds and 9 rounds.

Output tokens were 72.7% of the cost. Of the input tokens, 95.2% were read
from DeepSeek's cache. The tool definitions are about 3,000 tokens in every
round, and they are part of that cached prefix. Over the 1,227 rounds they
cost about $0.008, or about 2.6% of the total. Shorter definitions would save
under 1% and could change how the model uses the tools, so they stay as they
are.

`EFFORT=low` did not change the output. Over the first 19 runs, output went
from 63,463 to 66,530 tokens and time from 428 to 456 seconds. The rest of
that run was stopped.

A second set of 10 single-turn tasks, one run each, recorded the time of each
round and each tool call:

| Part of the turn | Share of 548 seconds |
|---|---|
| Model streaming, time to first token included | 73.1% |
| Time to first token alone, about 0.9 seconds a round | 16.3% |
| Tool calls | 26.7% |
| Socium itself | 0.2% |

Of the tool time, 142 seconds were `slow-events`, where the agent ran the
task's slow reference code. Without that task, tools took 1% to 5% of each
turn. Socium's own time is about 11 ms a round. A faster Socium could save at
most that 0.2%.

The raw numbers are `eval-ds-flash.json` and `time-split.json` in
[review-evidence/2026-11-19](review-evidence/2026-11-19/).
