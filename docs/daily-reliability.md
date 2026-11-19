# Daily workflow reliability

The first reliability milestone covers: read, search, edit, run
tests, cancel, and continue after restarting. The offline acceptance scenario is
in `crates/core/src/reliability_tests.rs`; it uses a scripted provider and temporary
Git repositories, with no API keys or paid requests.

## Runtime contract

- File tools accept relative workspace paths. Missing path components are
  normalized and existing symlinks are resolved before any directories are made.
  `.git` and `.socium` are protected from file mutations, including undo and aliases.
- Commands run in a required **Linux OS sandbox**. By default each waits for a
  human to approve the exact request and displayed policy in the TUI, once or,
  for its command prefix, for the rest of the session; `approval.commands =
  "auto"` and `approval.allow` rules run commands unasked in a narrower sandbox
  (see [configuration](configuration.md#sandbox-and-approval-settings)). An approval
  wait has no time limit, and approval cannot be supplied by model arguments.
  Headless callers without an approval handler (`-p`) refuse what needs
  approval. Children receive a fixed environment allowlist, private HOME, and
  no network access. See the [sandbox guide](linux-sandbox.md).
- Esc or Ctrl+C cancels a running request, approval wait, search, or command. The
  TUI waits for cleanup before accepting another submission. Ctrl+C when idle exits.
  A worker panic returns an error; a closed result channel tells the user to restart.
- Cancellation terminates the sandbox PID namespace, including detached descendants. Memory capture is
  capped at 256 KiB per stream, keeping the first 32 KiB and the newest output. `run_command` also
  saves up to 8 MiB per stream under `.socium/outputs/`, reports truncation, and shows byte-count
  progress while running; the model sees at most 20,000 characters per stream, mostly from the end,
  and reads the rest back with `read_file` on the saved file and an offset. The output of the newest
  100 commands is kept; older files are deleted when a command starts, and reading one that is gone
  says so. A result cut this way, or beyond 100,000 characters, is marked `truncated` in Activity.
  Timeout cleanup drains for at most another 200 ms. OS-level uninterruptible I/O
  can still delay kernel cleanup. Unsupported platforms and unavailable sandbox
  backends are rejected; there is no host fallback.
- Search uses `rg --no-config --json`, preserving regex/smart-case, glob, Git ignore,
  default hidden-file, and no-follow-symlink behavior. `-e` separates the pattern
  from options. Search fails explicitly if rg is absent, its regex is invalid, its
  5-second deadline expires, or a matched path/content cannot be decoded as UTF-8.
  Capture limits are reported, not represented as a complete search. Matched lines
  are cut at 500 characters. A recursive listing uses `rg --files`, so ignored
  files and `.git` are left out; a plain listing shows one directory.

## Editing and Git

Patch preview and apply use the same validated plan. Missing updates, conflicting
adds/moves, overlapping paths and ambiguous hunks fail before any file changes.
Insertion-only hunks remain explicitly unsupported; use `text_editor insert`.
Preview and every edit result show a unified diff of the changed lines with three
lines of context (cut after 80 lines in the result); applying the previewed diff
to the original gives exactly the bytes written, line endings included. `Add File`
ends every line with a newline, `*** End of File` anchors a hunk at the end, and
an empty line in a hunk is a blank context line.

Each file replacement is atomic and preserves existing permissions; moves and
undo keep the Unix permission bits (SOC-040). Only those bits (`mode & 0o7777`)
are kept: a moved or restored file is a new file, so it gets the user and group
of the process that writes it, and ACLs, extended attributes and timestamps are
not carried over. Off Unix no mode is recorded. The mode is set on the
replacement before it takes the file's place, so content and mode change
together; a failure is reported like any other write failure, with the paths
already applied. Undo refuses a file whose mode was changed after the edit, so a
`chmod` by a person is never silently reverted. History written before modes
were recorded undoes as before, without restoring a mode. A whole multi-file operation is not
atomic. The full before/after record is durable before
changes begin. Failures list applied paths and retain an incomplete history record.
Patch and text-editor changes share undo (`/undo`, or `/undo <path>`): undo
restores the entire recorded edit only when current contents match the expected
post-edit contents, and tells the model it did. The history keeps the newest 200
finished edits, up to 16 MiB of saved file text; unfinished records are never
dropped. An incomplete
edit or undo requires inspection; it is never silently rolled back over newer work.

Git read-only tools reject configurations containing external content filters;
status/diff can otherwise launch those programs without command approval.
External diff/text conversion, signature helpers and fsmonitor are disabled,
submodule worktrees are ignored, and lazy object fetches are disabled. Use an
explicitly approved sandboxed command for those features, within its declared
filesystem and network scope. Global/system Git configuration is not loaded,
except that `git_commit` takes your global `user.name` and `user.email` when the
repository sets none.

`git_commit` defaults to the existing staged changes. Optional `paths` commits only
those files and preserves unrelated index entries. Explicit paths with staged
changes are rejected to protect partial staging; use staged-only mode after
reviewing the index. A new file named in `paths` is shown in full for approval and
added with intent-to-add; if the commit fails, the mark is removed. Socium never
runs `git add -A` as part of `git_commit`. Approval displays the author, the
requested diff, and existing index; changes during the approval wait invalidate
the request. Failed or
cancelled Git commands require checking HEAD/index before retrying because hooks
can modify workspace files within the sandbox. Networked hooks and external
metadata are unsupported.

Content checks detect ordinary external edits before apply/undo. They do not
provide a race-free filesystem sandbox against a process concurrently swapping
symlinks or writing between the final check and rename. Avoid concurrent edits to
the same files. The command sandbox does not isolate concurrent host processes
or change these in-process file-tool guarantees.

## Save and resume

State directories and files must not be symlinks; state files must be regular,
singly linked files owned by the current user. On Unix `.socium` directories
use mode 0700 and newly saved state uses 0600, including migrated snapshots.
These permissions do not encrypt data or protect it from the same OS user.
Pre-existing linked state now fails startup: back it up, inspect it, and copy
validated contents into ordinary files/directories before resuming.

One Socium process owns a workspace through an OS file lock held for the
service lifetime. The lock file can remain after exit: the OS lock, not file
existence, establishes ownership. Each conversation is a session kept in
`.socium/sessions/<ID>/`, where the ID is its UTC start time
(`YYYYMMDD-HHMMSS-xxxx`). Its conversation, summary and execution events share
one versioned snapshot, `conversation.json`, which is first written when the
conversation has something to save; the name given with `/rename` is saved in
it. Each save also writes `meta.json`, a cache of what the session list shows
(message count, first prompt, name, model) for that exact snapshot size and
modification time. The list trusts it only while both match and otherwise
reads the snapshot and rewrites it, so a missing, stale or broken cache costs
time, not data. Deleting a session from the list removes its directory
permanently, the records of its subagents included; the open session cannot be
deleted. Edit history, input history
and command output artifacts belong to the workspace, not to a session. Writes
use a same-directory temporary file, file sync, rename and (on Unix)
parent-directory sync. State is bounded at 64 MiB; reaching
the limit fails explicitly instead of silently dropping history, and `/clear`
starts a new conversation.

The user request, assistant tool intent, authorized execution boundary and each
result are saved independently of turn success. Events identify the turn by its
raw user-message offset, the round, tool-call ID, and monotonic event sequence.
These offsets remain stable because raw history is not truncated at the front.
`/clear` begins a new session with its own event sequence and leaves the previous
session's files unchanged. Summaries only
change the model's context projection; full tool results remain in the transcript.

The Activity pane's record is appended to the session's `activity.jsonl`, separate from
the snapshot, and the most recent 20 turns are shown again at startup. It holds
reasoning summaries, tool inputs, one-line result summaries, timings and token
use, but never full tool output or raw provider events. A failed append shows a
warning and does not fail the turn; a line cut off by a crash is skipped on
replay. At 16 MiB the file moves to `activity.1.jsonl`, replacing the previous
one.

State saved before sessions existed, `.socium/conversation.json` with
`activity.jsonl` and `activity.1.jsonl`, is moved into a session named after the
conversation's modification time on the next start, or by `-r` listing the
sessions. The files are renamed, never copied or rewritten, and the conversation
moves last, so a move cut short finishes on the next start. When a file of the
same name is already in that session, startup stops and names both files rather
than overwrite either; move one aside and start again. To go back to an older
Socium, move the three files back to `.socium/`.

Each subagent's conversation is saved in the session that started it, as
`.socium/sessions/<ID>/agents/<id>.json`, before its first request and at the
same checkpoints as the main conversation, so its tool intents are on disk
before they run. `/agents` lists and `resume` continues only the open session's
subagents. A record keeps every message; once a subagent's history no longer
fits, a summary saved in the record replaces the older messages in what is
sent, as for the main conversation. A record left `running` was cut off by a
crash; resuming it marks its unfinished tool calls unknown, as on restart. Only
the 100 most recently updated records of a session are kept; older ones are
removed when a new subagent starts in it.

Records saved before they belonged to a session, in `.socium/agents/`, move on
the next start (or `-r` listing) into the most recently saved session with
messages, the one `-c` continues. A record whose ID that session already has
stays in `.socium/agents/` rather than overwrite it; while no session has
messages, all of them stay.

Bubblewrap ties subprocess lifetime to Socium and isolates descendants in a PID
namespace. A crash after intent but before a saved result still produces an **unknown outcome** on
restart. Socium does not re-execute that tool automatically. Inspect the files,
command output artifacts, Git HEAD/index, and any external system before asking
for a retry. Exactly-once external side effects cannot be guaranteed across a crash.

Only complete streams with a valid block sequence, stop reason and `message_stop`
can release tool calls. Truncated/token-limited responses fail explicitly. Context
trimming preserves tool-use/result groups and treats the historical summary as
user-level data. A request that still exceeds the estimated budget is rejected;
provider-specific token estimation and automatic in-loop compaction remain future
work in SOC-017.

## Recovery and migration

On startup, malformed JSON, unsupported schema versions and writer conflicts are
errors. They do not start an empty conversation and overwrite the evidence.

1. Stop all Socium processes for that workspace.
2. Copy `.socium` to a backup outside the working tree before changing anything.
3. Inspect `sessions/*/conversation.json`, `edit_history.json`, leftover
   `.socium-*.tmp` files and `outputs/`. A temporary snapshot may be older or
   newer; validate it before selecting it for manual recovery. Never infer a tool
   succeeded solely from intent.
4. Restore a validated complete snapshot from a backup, or move the saved state
   aside to intentionally start fresh. Do not delete a lock file to bypass a live
   writer. For incomplete edits, compare each recorded `before`/`after` to disk and
   recover explicitly. Automatic undo refuses uncertain records.

Legacy raw conversation messages are retained. Legacy separately stored summaries
are left untouched and not reused because their offsets may have become stale.
Legacy edit entries are preserved in JSON, but cannot be safely undone without
post-edit content; new edits use versioned grouped records. No automatic history
deletion is performed; command output files are limited to the newest 100
commands. Archive a stopped session manually when disk usage warrants it. Add `.socium/` to the project's `.gitignore`.

Validation targets Linux. Live model/network behavior and macOS execution have
not been exercised by the offline tests. Web search (SOC-037) came later and is
not part of this milestone.
