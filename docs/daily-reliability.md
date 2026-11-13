# Daily workflow reliability

The first reliability milestone covers: read, search, edit, run
tests, cancel, and continue after restarting. The offline acceptance scenario is
in `crates/core/src/reliability_tests.rs`; it uses a scripted provider and temporary
Git repositories, with no API keys or paid requests.

## Runtime contract

- File tools accept relative workspace paths. Missing path components are
  normalized and existing symlinks are resolved before any directories are made.
  `.git` and `.socium` are protected from file mutations, including undo and aliases.
- Commands run in a required **Linux OS sandbox**, after a human approves the
  exact request and displayed policy in the TUI. Approval is single use, expires
  after 120 seconds, and cannot be supplied by model arguments. Headless callers
  without an approval handler deny commands. Children receive a fixed environment
  allowlist, private HOME, and no network access. See the [sandbox guide](linux-sandbox.md).
- Esc or Ctrl+C cancels a running request, approval wait, search, or command. The
  TUI waits for cleanup before accepting another submission. Ctrl+C when idle exits.
  A worker panic returns an error; a closed result channel tells the user to restart.
- Cancellation terminates the sandbox PID namespace, including detached descendants. Memory capture is
  capped at 256 KiB per stream. `run_command` also saves up to 8 MiB per stream under
  `.socium/outputs/`, reports truncation, and shows byte-count progress while running.
  Timeout cleanup drains for at most another 200 ms. OS-level uninterruptible I/O
  can still delay kernel cleanup. Unsupported platforms and unavailable sandbox
  backends are rejected; there is no host fallback.
- Search uses `rg --no-config --json`, preserving regex/smart-case, glob, Git ignore,
  default hidden-file, and no-follow-symlink behavior. `-e` separates the pattern
  from options. Search fails explicitly if rg is absent, its regex is invalid, its
  5-second deadline expires, or a matched path/content cannot be decoded as UTF-8.
  Capture limits are reported, not represented as a complete search. File listing
  bounds traversal by time, entries, and depth; it does not apply Git ignore rules.

## Editing and Git

Patch preview and apply use the same validated plan. Missing updates, conflicting
adds/moves, overlapping paths and ambiguous hunks fail before any file changes.
Insertion-only hunks remain explicitly unsupported; use `text_editor insert`.
Preview shows complete before/after lines, including duplicate lines and missing
final newlines. It does not claim to be a minimal unified diff.

Each file replacement is atomic and preserves existing permissions. Move targets
and undo of deleted files currently restore content without restoring the original
Unix executable bit; metadata preservation is tracked in SOC-040 / issue #93. A whole
multi-file operation is not atomic. The full before/after record is durable before
changes begin. Failures list applied paths and retain an incomplete history record.
Patch and text-editor changes share undo: undo restores the entire recorded edit
only when current contents match the expected post-edit contents. An incomplete
edit or undo requires inspection; it is never silently rolled back over newer work.

Git read-only tools reject configurations containing external content filters;
status/diff can otherwise launch those programs without command approval.
External diff/text conversion, signature helpers and fsmonitor are disabled,
submodule worktrees are ignored, and lazy object fetches are disabled. Use an
explicitly approved sandboxed command for those features, within its declared
filesystem and network scope. Global/system Git configuration is not loaded.

`git_commit` defaults to the existing staged changes. Optional `paths` commits only
those tracked files and preserves unrelated index entries. Explicit paths with
staged changes are rejected to protect partial staging; use staged-only mode after
reviewing the index. New files must be staged explicitly. Socium never runs
`git add -A` as part of `git_commit`. Approval displays the requested diff and
existing index; changes during the approval wait invalidate the request. Failed or
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

One Socium process owns a workspace session through an OS file lock held for the
service lifetime. The lock file can remain after exit: the OS lock, not file
existence, establishes ownership. Conversation, summary and execution events share
one versioned snapshot. Writes use a same-directory temporary file, file sync,
rename and (on Unix) parent-directory sync. State is bounded at 64 MiB; reaching
the limit fails explicitly instead of silently dropping history.

The user request, assistant tool intent, authorized execution boundary and each
result are saved independently of turn success. Events identify the turn by its
raw user-message offset, the round, tool-call ID, and monotonic event sequence.
These offsets remain stable because raw history is not truncated at the front.
`/clear` begins a fresh conversation and clears its event sequence. Summaries only
change the model's context projection; full tool results remain in the transcript.

The Activity pane's record is appended to `.socium/activity.jsonl`, separate from
the snapshot, and the most recent 20 turns are shown again at startup. It holds
reasoning summaries, tool inputs, one-line result summaries, timings and token
use, but never full tool output or raw provider events. A failed append shows a
warning and does not fail the turn; a line cut off by a crash is skipped on
replay. At 16 MiB, and on `/clear`, the file moves to `activity.1.jsonl`,
replacing the previous one.

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
3. Inspect `conversation.json`, `edit_history.json`, leftover `.socium-*.tmp` files
   and `outputs/`. A temporary snapshot may be older or newer; validate it before
   selecting it for manual recovery. Never infer a tool succeeded solely from intent.
4. Restore a validated complete snapshot from a backup, or move the saved state
   aside to intentionally start fresh. Do not delete a lock file to bypass a live
   writer. For incomplete edits, compare each recorded `before`/`after` to disk and
   recover explicitly. Automatic undo refuses uncertain records.

Legacy raw conversation messages are retained. Legacy separately stored summaries
are left untouched and not reused because their offsets may have become stale.
Legacy edit entries are preserved in JSON, but cannot be safely undone without
post-edit content; new edits use versioned grouped records. No automatic history
or artifact deletion is performed. Archive output files or a stopped session
manually when disk usage warrants it. Add `.socium/` to the project's `.gitignore`.

Validation targets Linux. Live model/network behavior and macOS execution have
not been exercised by the offline tests. Web search is a separate planned feature
in SOC-037 / issue #90, not implemented by this milestone.
