# Evaluation tasks

Fixed tasks for measuring a change to prompts, tools, or limits against the
same requests. Each task in `tasks/` has a `repo/` copied into a fresh Git
repository, a `prompt`, and a `check` that passes only when the work is done.
`run.py` runs Socium on each with `socium -p` (ADR 17) and records the
result, rounds, tools, and tokens.

```sh
nix develop -c python3 evals/run.py --binary target/debug/socium --out results.jsonl
```

Every task calls the model you configured, so a run costs money. It uses
Socium's normal environment (`MODEL`, keys, `models.toml`) and, unless
`SOCIUM_CONFIG_FILE` is set, `evals/config.toml`, which lets commands run
without asking. Compare `results.jsonl` files from before and after a change;
one run per side says little, so repeat each a few times.

| Task | What it measures |
|---|---|
| `fix-failing-test` | Finding a bug and verifying the fix with `cargo test` inside the sandbox |
| `cite-location` | Citing an exact `path:line` from numbered reads |
| `crlf-edit` | Editing one line of a CRLF file without touching the others |
| `tail-failure` | Finding a failure printed at the end of a long command output |
| `late-requirement` | Keeping a requirement stated at the end of a long request |

The checks were confirmed to pass on a hand-made correct solution and to fail
on the untouched repository. The tasks have not yet been run against a real
model.

Add a task as a directory with `task.toml` and `repo/`. The check runs in the
repository after the agent finishes, on the host, with the agent's final reply
in the file named by `EVAL_REPLY_FILE`.
