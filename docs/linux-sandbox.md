# Linux command sandbox

SOC-003 / [#8](https://github.com/chansuke/socium/issues/8) separates application
authority (`ExecutionPolicy`) from enforcement (`SandboxExecutor`). Executable
names never grant permissions. The model cannot request additional paths,
environment variables, network access, or an unconfined mode. You can add
read-only directories, PATH entries, variables, and a longer time limit in
`~/.config/socium/config.toml` (see [User settings](#user-settings)); network
access cannot be added.

## Installation and support

Use Linux x86_64 or aarch64 with bubblewrap 0.12+ and enabled unprivileged user
namespaces. `nix develop` and the Linux Nix application supply bubblewrap. Native
installations also need Git and ripgrep. The TUI executes a harmless sandboxed
probe at startup while the interface opens, and no turn starts before it passes;
missing tools, disabled namespaces and invalid mounts stop startup. There is no
automatic or opt-in host-execution fallback in this version.
Linux x86_64 is exercised locally and in CI; aarch64 has an implementation but
has not been exercised on hardware. macOS and Windows are unsupported.

Run from a dedicated project directory. The project must not overlap the
runtime mounts, `/proc`, `/dev`, or `/socium-home`, and cannot be `/tmp` itself.
An ordinary root `.git` directory is supported, and so is a linked worktree made
by `git worktree add`. For a worktree, Socium checks every link Git creates: the
gitfile names a directory under `<main>/.git/worktrees/`, that directory's
`gitdir` points back to this gitfile, and its `commondir` names the main Git
directory, which holds `objects` and `HEAD` and lies outside the worktree. Only
then is the main Git directory mounted (read-only, or read/write during
`git_commit`); the gitfile itself is always read-only and the main checkout's
files stay invisible. Submodule gitfiles, symlinked Git metadata, and a
`commondir` in an ordinary `.git` are refused. Do not use your home directory as
the workspace.

The backend must report version 0.12.0 or newer and must not be setuid/setgid.
Older or unrecognized versions are rejected, even if a distribution claims a
backport. The Nix environment pins the upstream 0.12.0 release archive by hash
to include the fix for [CVE-2026-87766](https://github.com/containers/bubblewrap/security/advisories/GHSA-pxhw-h44j-8pfx).

Ubuntu's AppArmor user-namespace restriction can reject a Nix-installed backend
with `loopback: Failed RTM_NEWADDR: Operation not permitted`. An administrator
must allow `userns` for that exact trusted bubblewrap executable using an
AppArmor application profile; installing Nix alone does not grant this host
permission. See [Ubuntu's namespace policy](https://documentation.ubuntu.com/release-notes/24.04/).
CI installs a temporary profile for the pinned Nix store executable and unloads
it after testing. It never disables AppArmor globally or skips enforcement tests.

A profile for one exact store path stops working when a flake update changes
the hash, until it is installed again. A glob keeps working across updates:

```
# /etc/apparmor.d/socium-nix-bwrap
abi <abi/4.0>,
include <tunables/global>

profile socium-nix-bwrap /nix/store/*-bubblewrap-*/bin/bwrap flags=(unconfined) {
  userns,
}
```

Load it with `sudo apparmor_parser -r /etc/apparmor.d/socium-nix-bwrap`. The
glob lets every bubblewrap in the Nix store create user namespaces, including
versions Socium refuses to run; the exact path allows only the pinned one.

## Policy

| Operation | Workspace | Existing root `.git` | Root `.socium` | Network |
|---|---|---|---|---|
| Search, symbol search, Git queries | Read-only | Read-only | Hidden, except `scratch/` (read-only) | Denied |
| Approved `run_command` | Read/write | Read-only | Hidden, except `scratch/` (read/write) | Denied |
| `run_command` nobody reviewed (`approval.commands = "auto"` or an `allow` rule) | Read/write, except protected paths (read-only) and sensitive files (read as empty) | Read-only | Hidden, except `scratch/` (read/write) | Denied |
| Approved `git_commit`, including hooks | Read/write | Read/write | Hidden, except `scratch/` (read/write) | Denied |

Directories in `sandbox.read_only` are read-only in every row. A command
nobody reviewed gets the narrower view only when the protected and sensitive
files can be listed in time (at most 256 paths); otherwise it asks. When it
ends, the protected files are listed again, symlinks included; any it created
or replaced, such as a new `.envrc` or a workflow written after moving
`.github` aside, is moved to `.socium/quarantine`, and the command reports
them as an error.

Every tool subprocess goes through this executor. The trusted backend version
query runs outside the sandbox with an empty environment before setup. Human approval displays
the policy and exact requested arguments and remains mandatory for commands and
commits. The parent handles model HTTP requests and writes session state/output
artifacts; provider credentials never need to enter the child environment.

The sandbox starts with an empty filesystem view. The canonical project is bound
at its original absolute path. Available `/usr`, `/bin`, `/sbin`, `/lib`, `/lib64`,
and `/nix/store` are read-only runtime mounts. `/etc/ld.so.cache` is the only host
`/etc` file mounted. `/proc` belongs to the new PID namespace, `/dev` contains
bubblewrap's private devices, and `/tmp` and `/socium-home` are fresh tmpfs mounts
per command. The existing state directory is masked by an empty read-only mount
that holds only `.socium/scratch/`, the agent's directory for temporary files,
which keeps its contents between commands.
The backend and bare executable names resolve through canonical absolute system
runtime PATH directories; relative/current-directory PATH entries are discarded.
Use `./program` to explicitly execute a workspace program. Its permissions are
identical to those of a system executable.

The child environment is rebuilt with only:

- `PATH`: canonical system runtime directories from the launcher's PATH, plus
  available system bin directories, then the `sandbox.path` entries that lie in
  a mounted directory; no other home or workspace PATH entries. The backend
  itself is found only in the system directories.
- `HOME=/socium-home`, `TMPDIR=/tmp`, `LANG=C.UTF-8`, `LC_ALL=C.UTF-8`, `TERM=dumb`.
- `GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_GLOBAL=/dev/null`,
  `GIT_TERMINAL_PROMPT=0`, `GIT_NO_LAZY_FETCH=1`.

Variables from `[sandbox.env]` come last; PATH, HOME, TMPDIR and `GIT_*` cannot
be set there. bwrap reads them from a descriptor and sets them in the command
only, so a loader variable such as `LD_PRELOAD` does not reach bwrap itself,
which runs on the host, and the values do not appear in its command line. There
is no inherited credential denylist to become stale: unknown
future secret variables are excluded as well. `git_commit` uses the repository's
identity, or else your global `user.name` and `user.email`, which the host's Git
reads outside any repository and passes in as `GIT_AUTHOR_*`/`GIT_COMMITTER_*`.
Global hooks, aliases, credential helpers and filters are not inherited.

Bubblewrap creates user, mount, PID, IPC, UTS and network namespaces, drops all
capabilities, disables nested user namespaces and ties child lifetime to the
parent. A seccomp filter rejects alternate syscall ABIs and denies socket
creation for all address families, io_uring, keyring access, ptrace/process-memory
access, pidfd descriptor theft, BPF and performance monitoring. Network namespaces
alone would not block host pathname Unix sockets in a mounted workspace; the
socket filter covers that path too. `socketpair` is allowed for `SOCK_STREAM` and
`SOCK_SEQPACKET` (with or without `SOCK_CLOEXEC`/`SOCK_NONBLOCK`): such a pair is
connected to its own peer and has no address, and Rust's process spawning,
Python's `asyncio`, and Node's `child_process` need one. `SOCK_DGRAM` pairs stay
denied, since `sendto` on one could reach a host pathname socket.
Timeout/cancellation kills the sandbox, so `setsid` does not escape cleanup.

## Practical limits

Networked package installation, Git fetch/push, local TCP test servers and daemon
sockets are intentionally unavailable. Perform those steps yourself outside the
agent. A home-installed rustup/npm toolchain, user package caches and signing keys
are not exposed unless you list them in `sandbox.read_only`. Commands cannot
stage files in an existing `.git`; `git_commit` adds new files named in `paths`
with intent-to-add and removes that mark again if the commit fails.

Workspace contents are still readable by the model through file tools and command
output. `.gitignore` is not a confidentiality boundary. A command may damage any
workspace content its policy permits. Existing hardlinks can alias files outside
the workspace or protected metadata, so use trusted checkouts without such links.
This is not a VM, does not set CPU/memory/disk quotas, and does not defend against
a compromised kernel or hostile processes already running as the host user.
Path-based setup and the parent's file tools are not a race-free boundary against
concurrent host filesystem modification. Cancellation does not undo completed
writes; inspect unknown outcomes before retrying after a crash.

## User settings

`[sandbox]` in `~/.config/socium/config.toml` (see
[`config.toml.example`](../config.toml.example)) widens what commands see:

- `read_only`: host directories mounted read-only at their own paths, after the
  fresh `/tmp` and before the workspace, so a listed ancestor of the workspace
  can neither make it read-only nor reveal `.socium`. `/`, the home directory
  and its ancestors, `/proc`, `/dev`, `/socium-home`, and `/tmp` itself are
  refused; a directory that does not exist is skipped with a warning. A
  symlink is mounted at both its target and its own path, so a symlink whose
  target contains the workspace or lies inside it is refused: list the target
  instead. Executables inside these directories may run.
- `path`: PATH entries, kept only when they lie in a `read_only` directory or a
  system runtime.
- `env`: extra variables, such as `CARGO_HOME`.
- `command_timeout`: the longest a command may run, 1 to 86400 seconds
  (default 120).

With `~/.cargo/registry`, `~/.cargo/git`, and `~/.rustup` listed and
`CARGO_HOME = "~/.cargo"`, `cargo build --offline` of a crates.io-dependent
project works from the host's cache.

## Verification

Run on a Linux host capable of starting the namespaces (not a container that
prohibits nested namespace creation):

```sh
nix develop --command cargo test --locked --all-targets --all-features
nix develop --command cargo clippy --locked --all-targets --all-features -- -D warnings
```

`crates/core/src/execution/tests.rs` builds and runs a Cargo project, Python
`asyncio` subprocesses, and Node `child_process` inside the sandbox, and checks
user mounts, PATH and variables, hidden and frozen paths, linked worktrees and
forged gitfiles, and allowed workspace writes alongside
denied outside reads/writes, `sed -i`, `env` wrappers, same-name workspace programs,
symlink escapes, hidden state, protected Git metadata, redirected-layout refusal,
fake credential exclusion, fresh HOME/tmp, TCP/UDP/Unix sockets, io_uring,
namespace re-entry, failed backend setup, confined Git hooks and detached-child
timeout cleanup. Existing reliability tests cover approval, cancellation,
restart/resume and preservation of unrelated edits through the same executor.

Design references: [bubblewrap](https://github.com/containers/bubblewrap) and
[Linux seccomp filter documentation](https://docs.kernel.org/userspace-api/seccomp_filter.html).
