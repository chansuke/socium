# Using Socium

Start `socium` in a project directory and ask for something in the composer.
This guide describes what the terminal UI does; see
[configuration](configuration.md) for models and sandbox settings.

Add `.socium/` to your project's `.gitignore`. Conversation, input history,
edits, and command artifacts are stored there without encryption.

Socium is told to keep temporary scripts and other helper files in
`.socium/scratch/`, which its file tools and commands can write, and to
remove any it makes elsewhere, as Anthropic's
[prompting guide](https://platform.claude.com/docs/en/build-with-claude/prompt-engineering/claude-prompting-best-practices)
suggests.

## Composer and commands

- Type `/` to show command suggestions: Up/Down select, Tab completes, Enter
  runs, Esc closes. `/commands` lists every command; `/help` adds key
  bindings. Type `exit` or `/exit` to quit.
- The composer takes pasted text whole, newlines included. Alt+Enter or
  Shift+Enter (where the terminal reports it), or `\` before Enter, starts a
  new line; Up/Down move between the lines of a message. You can write the next
  message while a request runs and send it once the request ends.
- Typing while a task runs adds to it: Enter hands your words to the main agent,
  which reads them once the tools it is running have finished, before its next
  request; Activity shows the step. Words it has not read when the turn ends are
  sent as the next turn, or put back in the input if the task was cancelled or
  failed. Commands still wait for the task to end.
- Press Esc or Ctrl+C during a task to cancel and wait for cleanup. In normal
  mode (after Esc) `q` no longer quits; use `/exit` or Ctrl+C.
- The status line shows how much of the input budget is left, the tokens the
  conversation has used and the share read from the prompt cache, and the cost
  when every model used has a `price`.
- Conversation, Activity, and code blocks wrap by terminal cells, so CJK text,
  combining characters, and emoji sequences are not cut off.

## Approvals

Review commands, commit diffs, and protected edits in the TUI: choose with the
arrow keys and Enter, or press a choice's number, `y` to allow once, `a` to
allow for the session (when offered), or `n` to deny. While a request waits,
the whole request, diff included, replaces the conversation: PgUp/PgDn (or
Space/b) page through it and Home/End jump to its ends; these keys never
answer.

Commands can write workspace files, but cannot access the host home or
network. Existing Git metadata is read-only except during `git_commit`, which
commits a symlink named in `paths` as a link, never the file it points to;
Socium's state is hidden from all child processes, except
`.socium/scratch/`, where commands may keep temporary files. What asks and what runs
without asking is set in [`config.toml`](configuration.md#sandbox-and-approval-settings).

## Files, edits, and command output

- Reads number every line and return 2,000 lines at a time, saying where to
  continue; long lines are cut. Searches that stop at their match limit say the
  result is incomplete.
- Each edit result, and the conversation, shows the edit's diff. `/undo`
  reverts Socium's last edit, and `/undo <path>` its last edit of that file;
  the history keeps the last 200 edits, up to 16 MiB.
- Patches keep each untouched line's LF or CRLF ending and refuse files with a
  stray carriage return; moves and undo keep Unix permission bits such as the
  executable bit (not owners, ACLs, or extended attributes), and undo refuses
  a file whose mode was changed since; FIFOs and other special files are
  refused rather than read.
- Command output beyond 20,000 characters keeps its start and, mostly, its end,
  where failures are reported; each stream is saved under `.socium/outputs/`
  (up to 8 MiB).

## Activity pane

The Activity pane shows the model's reasoning summaries and progress updates,
tool calls as their input is written, tool results and durations, and
per-round stop reason, token use, and time to first token. On terminals 140
columns or wider it sits beside the conversation, and recent activity is shown
again after a restart.

Press Ctrl+A to focus it: Up/Down select, Ctrl+L jumps to the top, Enter
expands, `r` shows the round's raw provider events, `z` fills the screen, Esc
returns to the input. Its title shows what the agent is doing (investigating,
editing, verifying), and it points out repeated calls, a change of course after
a failure, and long stretches of tool rounds without an update.

## Subagents

The model can hand a self-contained task to a subagent with the `agent` tool.
The subagent starts from an empty context and returns only its final report,
so a broad search stays out of the conversation. An `explorer` can only read,
list, and search files and inspect Git; a `worker` can also edit and run
commands, which still need your approval; the approval names the subagent that
asked. You can add your own types; see
[configuration](configuration.md#subagent-types).

- The report ends with the subagent's model requests, the tool calls it ran,
  token use, time, and its ID.
- A subagent runs on the current model unless its type or the call names
  another registered model whose API key is set, so a simple search can go to a
  smaller model.
- Subagents cannot start subagents, and the main agent waits for them. New
  subagents of types that cannot change the workspace (such as `explorer`),
  called one after another in the same response, run at the same time, tool
  calls included, up to four at once; others run one at a time.
- Each subagent's steps appear in the Activity pane under its own `agent` call,
  marked with `│`, and its repeated calls and silent rounds are counted apart
  from the other agents'.
- Each subagent's conversation is saved in the session that started it, as
  `.socium/sessions/<session>/agents/<id>.json`, before each of its tool calls.
  The model can continue a subagent of the same session with everything it saw
  by passing its ID as `resume`. `/agents` lists this session's subagents and
  `/agents <id>` shows one's conversation. The newest 100 of each session are
  kept, and deleting a session deletes them.
- When a subagent's history no longer fits its model's window, it is
  summarized as the conversation is (by its model's `compact_model`), shown in
  the Activity pane, and the subagent goes on and still reports; a resumed
  subagent is summarized first when it needs to be. A follow-up the subagent
  fails without answering is not kept, so it can be sent again.
- A restart during a subagent still marks the `agent` call's outcome unknown;
  the reports of subagents running with it that had already finished are kept.

See [ADR 8](adr/0008-agentツールで独立した文脈のサブエージェントに作業を委ねる.md).

## Sessions

- Each start opens a new conversation. `-c` (`--continue`) continues the most
  recently saved one in the workspace, and `-r` (`--resume`) lists the saved
  ones, newest first, to choose from; `-r <ID>` opens one directly, by its full
  ID, its name, or any unique ID prefix.
- `/resume` opens the same list between turns (`/resume <ID>` switches at
  once); in the list, `d` then `y` permanently deletes the selected
  conversation and its subagents' records. `/rename <name>` names the open
  conversation. `/clear` starts a new conversation and keeps the previous one
  resumable. On exit Socium prints the command that resumes the conversation.
- Resuming loads the conversation and Activity, and the conversation shows each
  tool call with its argument, a one-line result, whether it was approved, and
  failed requests; interrupted operations with no saved result are marked
  unknown, never replayed. Only one Socium runs in a workspace at a time.
- `/rewind` lists your prompts in this conversation, newest first. Choosing one
  drops it and everything after it and puts it back in the input to edit and
  send again. When Socium edited files from that prompt on, you choose whether
  to restore the conversation, the files, or both. Files are restored only if
  they still hold what Socium wrote; otherwise nothing changes. Edits whose
  turns were dropped by a conversation-only rewind stay for `/undo`.

## Long conversations and failures

- `/compact`, automatic compaction, and budget trimming replace older turns
  with a summary. Thinking in the turns kept verbatim was produced against the
  replaced history, so it is no longer sent back; thinking produced after the
  summary is. The full history stays in `.socium/` either way, and summary
  updates keep room for the newest messages. The first request is kept word
  for word beside the summary.
- One tool result holds at most 100,000 characters; when the results of a round
  would overflow the window, they are cut to fit, and if that is not enough the
  turn stops, the history is compacted, and you can ask Socium to continue.
- When the provider is overloaded, rate limits, or the connection fails or goes
  quiet for 180 seconds before any output, the request is retried up to three
  times; a long answer is never cut off while data keeps arriving. An answer
  that stops at the output limit is kept with a notice.

See the [reliability and recovery guide](daily-reliability.md) for storage
limits, editing guarantees, migration, and recovery procedures.

## Running without the TUI

`socium -p "<prompt>"` runs one turn without the terminal UI and prints the
reply text on standard output, without control characters other than tabs
and line breaks, so a reply cannot drive the terminal. Text piped into it comes before the prompt,
separated by a blank line, so `git diff | socium -p "review this"` works; the
piped text alone is also a prompt. Errors, notices, and diagnostics go to
standard error.

`--output-format` picks what standard output carries: `text` (the default),
`json` (only the final `finished` record), or `stream-json` (JSON Lines). In
JSON each line has the schema version in `v` and a `type`: `started`, then one
`activity` per Activity event, then `finished` with `ok`, the reply `text` or
the `error`, and the tokens used.

Nobody can answer an approval there, so a call that needs one is refused and
the model is told how to allow it; Ctrl+C cancels. The exit status is 0, 1 on
failure, 2 for a usage error, or 130 when cancelled. `-c` and `-r <ID>`
continue a saved conversation.
