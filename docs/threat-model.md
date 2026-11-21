# Threat model

What Socium protects, from whom, and what it leaves to you. The
[security policy](../SECURITY.md) states the same boundaries briefly; the
[Linux sandbox guide](linux-sandbox.md) gives the exact policy.

## What is trusted

- **You**, and what you type and approve.
- **Your configuration outside the workspace**: `~/.config/socium/config.toml`,
  `models.toml`, and the subagent definitions. Socium refuses to start in a
  workspace that contains them, so neither the model's file tools nor its
  commands can change them.
- **The host**: the kernel, the Socium binary, and the bubblewrap it runs.
- **The MCP token store** (`~/.local/state/socium/mcp-auth/`, or under
  `$XDG_STATE_HOME`), written by `socium mcp login`. It is host state of your
  user, kept owner-only and outside every workspace; Socium refuses to start
  in a workspace that contains it. Any process running as your user can read
  it, as it can your other credentials.
- **The MCP servers you configure** in `config.toml`. A local one runs on
  the host, outside the sandbox, with your network and files, unless you set
  `sandbox = "read-only"` for it. A workspace cannot add one.

## What is not trusted

- **The model's output.** It can be wrong, and it follows instructions it
  reads, including instructions planted in files, command output, or
  `AGENTS.md`. Nothing the model writes grants authority: a tool call cannot
  ask for a wider sandbox, approve itself, or choose its own permissions.
- **The workspace contents.** A cloned repository may be hostile. Its files
  are data to the model; its build scripts and Git configuration do not run on
  the host because Socium reads or edits them.
- **The model provider** sees everything sent to it: your prompts and every
  file, diff, and command output a tool returns. Register only endpoints you
  would show that content to.
- **A remote MCP server** (`url`) sees, like a model provider, everything
  sent to it: the arguments of each tool call the model makes, which can hold
  file contents, plus the headers you configured, usually a token, or the
  OAuth access token of your login. That token lets the server, and whoever
  holds it, act as you at the server's service within the scopes you agreed
  to in the browser, until it expires; the refresh token, which stays in the
  store, renews it. Register only servers you would show that content to.
- **A remote MCP server's authorization server**, found through the server's
  own metadata. It receives the login's code and verifier and issues the
  tokens. Socium checks that its metadata names itself as the issuer, that
  its endpoints use https (plain http only on loopback), and follows no
  redirect from it.
- **What MCP tools return.** A server's output goes to the model as it is and
  can carry planted instructions, like a file or command output.

## What is protected, and how

| Asset | Threat | Control |
|---|---|---|
| Host home and credentials (`~/.ssh`, cloud and Git credentials, keyrings) | A command reads them and the model sends them on | Commands run in bubblewrap with a private HOME and only the workspace, system runtimes, and the directories you list in `sandbox.read_only`. The home directory itself cannot be listed. |
| Provider API keys | A command or the model reads them | Keys stay in the parent process. Children get a fixed environment, never inherited variables; a local MCP server gets only the variables its `env` names, and a remote one only the headers in its table. |
| Tokens for remote MCP servers | They leak into logs, the activity record, or the model's context | Header values come from your environment through `${NAME}`, are marked sensitive in the HTTP client, and Socium puts them in no warning, error, startup notice, activity event, or prompt; a value or OAuth token the server writes back into an HTTP or JSON-RPC error, a tool error result, or a startup failure is blanked out before the quote is cut short. They travel only over https (plain http only to loopback, never through a proxy), and redirects are not followed. OAuth tokens, codes, PKCE verifiers, and client secrets follow the same rules; errors quote only the OAuth error code. |
| OAuth logins | A stolen or injected authorization code, or a token sent to the wrong server | PKCE with S256 (a server that does not list it is refused), a random `state` checked on return, a redirect URI on 127.0.0.1 only, and the `resource` parameter on both requests. A stored token is used only for the URL, the configured client, and the configured `issuer` it was issued for, a configured `Authorization` header replaces it, and a configured client secret goes only to the configured `issuer`. URLs the server names, other than its own well-known URLs, may not point at this machine or a private network unless its own URL does; a name is resolved once, refused if it resolves inside, and requests are pinned to the checked addresses and never go through a proxy. The authorization URL is checked the same way before the browser gets it, but the browser resolves it again itself. The browser opener is found on a PATH without the workspace, may not link into it, and gets only desktop variables, not `BROWSER`. A login saves, and renewals compare the whole stored record, under the renewal lock, so a renewal of an older login cannot overwrite a newer one. |
| The MCP token store | Commands or the model read it | It lives outside the workspace, where the file tools do not reach, and commands see it only if you list it in `sandbox.read_only`. It is a 0700 directory of 0600 files replaced atomically; links, files of other users, and a directory others can enter are refused when reading as well as writing. |
| The network | A command exfiltrates data or fetches code | Network namespace and seccomp deny all sockets except connected stream and seqpacket pairs, which reach only their own peer. Datagram pairs, which could reach host pathname sockets, are denied. |
| The host itself | An edit makes the host run code later: direnv loads `.envrc`, rust-analyzer runs `build.rs` and the build scripts and proc macros of the crates `Cargo.toml` and `Cargo.lock` pick, CI and hook managers run their files | Edits to these files always ask for approval, with the diff (`approval.protected`). Commands nobody reviewed run with them read-only, and a protected file such a command creates or replaces is moved to `.socium/quarantine`, or removed, when it ends. |
| Secrets in the workspace (`.env`, keys) | The model reads them and they reach the provider | Reads by file tools, and searches and diffs that name them, always ask (`approval.sensitive`). Wider searches and diffs leave them out and say which. Commands nobody reviewed see them as empty files. An approved command can still read them. |
| Git history and metadata | A command rewrites history, hooks, or config; the model reads credentials in `.git/config` | `.git` is read-only except during `git_commit`, which always asks and shows the diff, the new files, and the author. A linked worktree's main Git directory is mounted only after its back links are checked. |
| The workspace, network, and secrets, from a sandboxed MCP server (`sandbox = "read-only"`) | Its tools run without asking, so a server that misbehaves, or the model through it, writes the workspace, exfiltrates, or reads secrets | The server runs in the sandbox with the workspace read-only, no network (the same namespace and seccomp filter as commands), a private HOME and `/tmp`, and only the system runtimes and its own `read_only`, none of `[sandbox]`. `read_only` may not overlap the workspace, `.socium`, or Socium's configuration and tokens. Files matching `approval.sensitive` when it starts are covered by `/dev/null`, which it cannot open, under every name it could open them by (hard links, paths through directory symlinks, a `.socium` below the root, `.socium/scratch`); when they cannot all be found, it does not start. `read_only` is checked again at start and mounted from where it was checked, and its program must resolve into the runtimes or `read_only`, never the workspace. Not hidden: `.git` and a linked worktree's main Git directory (credentials in them are visible), and secrets created after it started. It sees more than `read_file`, and what its tools return goes to the provider. |
| Socium's own state (`.socium/`) | A command forges history, approvals, or settings; the model reads past sessions and sends them on | Hidden from every command. File tools read only saved command output and `.socium/scratch`. MCP servers you configure run on the host and are not confined, except sandboxed ones, which see only `.socium/scratch`, read-only. |
| Your uncommitted work | The model overwrites or loses it | Edits are checked against the file's current content, recorded before they are written, and can be undone with `/undo`. Commits cover only the paths named in the request. |

## What approval is for

Approval exists for actions whose effect reaches beyond the sandbox or is hard
to undo: commands (by default), commits, edits to files the host runs, reads of
secrets, and MCP tool calls (unless the server's `allow` lists the tool, or
the server runs in the read-only sandbox, which confines it instead). It is
not a substitute for the sandbox: an approved command runs with the same
isolation, and no answer widens it.

`approval.commands = "auto"` and `approval.allow` trade a prompt for a narrower
sandbox, not for trust. Choose them for commands whose worst case inside the
workspace you accept, such as `cargo test`.

`--dangerously-skip-permissions` gives up the reviews as well. The model can then
edit files the host runs later, such as `.envrc` or `build.rs`, and read likely
secrets, which reach the model provider, and call any tool of a host or remote
MCP server, which nothing confines. The sandbox still holds while Socium
runs, but a host tool that later runs an edited file does so outside it. Use
the flag only where you would throw the workspace away.

## What Socium does not protect against

- A command you approve damaging workspace files it may write, or reading a
  secret file you let it read.
- Host tools that run workspace files not on the protected list. Add them to
  `approval.protected`.
- The workspace's own code that rust-analyzer runs. It builds the proc-macro
  crates and build scripts of the workspace, so an edit to their sources
  (`src/lib.rs` of a proc-macro crate, or a build script named in
  `Cargo.toml`) runs on the host when the editor loads it. Only `build.rs` and
  `Cargo.toml` ask. Turn off rust-analyzer's build scripts and proc macros, or
  review such edits, in a workspace you do not trust.
- A command that runs without asking changing the file a protected name links
  to, such as `manifest.toml` behind a `Cargo.toml` link. A protected file it
  creates or replaces is moved to `.socium/quarantine` only when it ends, so a
  host tool watching the workspace can still run it while the command runs.
  A protected file your editor saves while such a command runs is moved too.
- Instructions planted in the workspace that persuade the model to propose
  harmful but sandboxed work. Read approval requests before answering.
- Hardlinks into the workspace, a compromised kernel, hostile processes running
  as your user, and resource exhaustion. This is not a VM.
- What the provider does with the data it receives.
- What an MCP server does, on the host or at its remote address. Socium
  decides only whether a tool call is sent to it, not what the server does
  with it. A sandboxed server is confined, but can still read what it sees
  and return it to the model. A local server on the host starts in `/` and finds its program on a PATH
  without the workspace (a sandboxed one starts in the workspace root, which it sees read-only), but Socium cannot stop a server you configured from
  reading the workspace or a value you gave it, or from writing that value
  into a tool result.
