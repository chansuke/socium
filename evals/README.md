# Evaluation tasks

Fixed tasks for measuring a change to prompts, tools, or limits against the
same requests. Each task in `tasks/` has a `repo/` copied into a fresh Git
repository, one or more requests, a check the agent never sees, and the paths
the task may change. `run.py` runs Socium on each with `socium -p` (ADR 17),
continuing the same session with `-c -p` for tasks with several turns.

A run passes only when its check passes and nothing outside the task's
`allowed_changes` changed. The agent's own report of success is not used.

## Running

Every real run calls the model you configured and costs money, so it needs a
budget. The model must have a `price` in `models.toml`; without one the cost
is unknown and the run stops after the first task.

```sh
nix develop -c python3 evals/run.py --binary target/release/socium \
  --budget-usd 2 --repeat 3 --label before --out before.jsonl
nix develop -c python3 evals/compare.py before.jsonl after.jsonl
```

No run starts once the money spent plus the dearest run so far would pass
the budget, and each run is stopped (as with Ctrl+C) at its `max_tokens`
(400,000 by default, or `--max-tokens`) or its `timeout`. The budget can
therefore be passed by at most one run. Runs that were not started are
counted as not run, never as failures or passes.

It uses Socium's normal environment (`MODEL`, keys, `models.toml`) and,
unless `SOCIUM_CONFIG_FILE` is set, `evals/config.toml`, which lets commands
run without asking. Nobody can answer an approval in `-p`, so `interventions`
is always 0 and work that needed a person shows as `approvals_refused`.

## Results

The first line of the output is a manifest with what a rerun must keep
fixed: the harness commit and whether it had local changes, the binary,
config and models file hashes, the model, the budget, the token cap, the
repeat count, and a hash of every task directory. Socium sends no sampling
seed, so repetitions stand in for one. Each result line holds, per task and
repetition:

| Field | Meaning |
|---|---|
| `passed` | the check passed and there were no unintended changes |
| `check_passed`, `check_output` | the check alone, and its last output |
| `changed`, `unintended_changes` | paths changed, and those outside `allowed_changes` |
| `tool_errors`, `provider_retries`, `recovered` | failures during the run, and whether it passed anyway |
| `approvals_refused`, `interventions` | calls that needed a person |
| `usage`, `cost_usd`, `unreported_calls` | tokens and cost; with unreported calls they are lower bounds |
| `seconds`, `rounds`, `tools`, `turns_sent`, `stopped` | time and shape of the run |

A `halted` line records why a set stopped early.

## Comparing

`compare.py` calls one side better only when the 95% Wilson intervals of
the two pass rates do not overlap. With a few runs per task that rarely
happens, and it reports "no clear difference" with the counts. It refuses
tasks whose files changed between the runs and lists tasks run on one side
only.

Run each side with `--comparison controlled` (the default) to compare
harnesses or changes with the same model, endpoint, budget, and token cap;
`compare.py` refuses the pair if those differ. Use `--comparison defaults`
for each harness as it ships, where they may differ and the report lists
which do. Keep the two kinds in separate files. Only Socium is wired in
today; another harness would need its own runner that writes the same
records.

## Checking the evaluation itself

```sh
nix develop -c python3 evals/run.py --self-check
nix develop -c bash -c 'cargo build && python3 -m unittest evals/test_evals.py'
```

`--self-check` confirms that every check fails on the untouched repository
and passes on the task's `solution/` without unintended changes. The tests
run the real binary against `mock_provider.py`, a loopback model that writes
the solution, does nothing, or adds a stray file, and confirm the pass,
fail, scope, budget, token cap, and comparison rules. CI runs them.

## Tasks

| Task | Category | What it measures |
|---|---|---|
| `cite-location` | repo-understanding | Citing an exact `path:line` from numbered reads |
| `understand-precedence` | repo-understanding | Following a value through several files |
| `count-callers` | repo-understanding | Counting call sites, past comments, strings, and longer names |
| `fix-failing-test` | bug-fix | Finding a bug and verifying the fix with `cargo test` in the sandbox |
| `fix-pagination` | bug-fix | Fixing code, not tests, with hidden edge cases |
| `fix-chunk` | bug-fix | A bug from a report, with no failing test |
| `fix-duration` | bug-fix | Fixing one input form without breaking the others |
| `refactor-extract` | refactor | Extracting a helper with the same behavior and messages |
| `refactor-rename` | refactor | Renaming across code, tests, and docs |
| `refactor-constant` | refactor | Naming a repeated value, leaving an equal one that means something else |
| `add-tests` | tests | Tests judged by whether they catch broken versions of the code |
| `regression-test` | tests | A test that fails before the fix and passes after it |
| `crlf-edit` | existing-change | Editing one line of a CRLF file without touching the others |
| `dirty-worktree` | existing-change | Working beside the user's uncommitted changes |
| `add-cli-flag` | existing-change | Extending a command without changing its current output |
| `late-requirement` | long-request | Keeping a requirement stated at the end of a long request |
| `tail-failure` | long-output | Finding a failure printed at the end of a long command output |
| `conversation-accumulate` | long-conversation | Requests that build on each other over three turns |
| `conversation-early-rule` | long-conversation | A rule from the first turn that later turns do not repeat |
| `changed-format` | requirement-change | A requirement replaced mid-conversation |
| `narrowed-scope` | requirement-change | A wider change narrowed, and partly undone, in a later turn |

The tasks have not yet been run against a real model.

## Adding a task

Add a directory with `task.toml`, `repo/`, and either `solution/` (the files
a correct run leaves) or `reply.txt` (a correct final reply), and run
`--self-check`. A check that needs more than one line goes in `check.py`
next to `task.toml`, outside the repository, and is named as `{task}/check.py`.
`dirty/` holds uncommitted changes written over `repo/` after its commit.
The fields are described at the top of `run.py`.
