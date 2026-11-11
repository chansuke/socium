# Linux command sandbox

SOC-003 / [#55](https://github.com/chansuke/socium/issues/55) separates application
authority (`ExecutionPolicy`) from enforcement (`SandboxExecutor`). Executable
names never grant permissions. The model cannot request additional paths,
environment variables, network access, or an unconfined mode.

## Installation and support

Use Linux x86_64 or aarch64 with bubblewrap 0.12+ and enabled unprivileged user
namespaces. `nix develop` and the Linux Nix application supply bubblewrap. Native
installations also need Git and ripgrep. The TUI executes a harmless sandboxed
probe at startup; missing tools, disabled namespaces and invalid mounts stop
startup. There is no automatic or opt-in host-execution fallback in this version.
Linux x86_64 is exercised locally and in CI; aarch64 has an implementation but
has not been exercised on hardware. macOS and Windows are unsupported.

Run from a dedicated project directory. The project must not overlap the
runtime mounts, `/proc`, `/dev`, or `/socium-home`, and cannot be `/tmp` itself.
An ordinary root `.git` directory is supported. Gitfiles, linked worktrees,
symlinked Git metadata and shared `commondir` layouts are refused. Granting their
external metadata requires a separate design; copying a self-contained checkout
is the supported alternative. Do not use your home directory as the workspace.

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

## Policy

| Operation | Workspace | Existing root `.git` | Root `.socium` | Network |
|---|---|---|---|---|
| Search, symbol search, Git queries | Read-only | Read-only | Hidden | Denied |
| Approved `run_command` | Read/write | Read-only | Hidden | Denied |
| Approved `git_commit`, including hooks | Read/write | Read/write | Hidden | Denied |

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
per command. The existing state directory is masked by an empty read-only mount.
The backend and bare executable names resolve through canonical absolute system
runtime PATH directories; relative/current-directory PATH entries are discarded.
Use `./program` to explicitly execute a workspace program. Its permissions are
identical to those of a system executable.

The child environment is rebuilt with only:

- `PATH`: canonical system runtime directories from the launcher's PATH, plus
  available system bin directories; no arbitrary home or workspace PATH entries.
- `HOME=/socium-home`, `TMPDIR=/tmp`, `LANG=C.UTF-8`, `LC_ALL=C.UTF-8`, `TERM=dumb`.
- `GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_GLOBAL=/dev/null`,
  `GIT_TERMINAL_PROMPT=0`, `GIT_NO_LAZY_FETCH=1`.

There is no inherited credential denylist to become stale: unknown future secret
variables are excluded as well. Configure Git identity with repository-local
`git config user.name` and `git config user.email` outside Socium before use.
Global hooks, aliases, credential helpers and filters are not inherited.

Bubblewrap creates user, mount, PID, IPC, UTS and network namespaces, drops all
capabilities, disables nested user namespaces and ties child lifetime to the
parent. A seccomp filter rejects alternate syscall ABIs and denies socket
creation for all address families, io_uring, keyring access, ptrace/process-memory
access, pidfd descriptor theft, BPF and performance monitoring. Network namespaces
alone would not block host pathname Unix sockets in a mounted workspace; the
socket filter covers that path too, including `socketpair`, whose Unix datagrams
could otherwise reach host pathname sockets. Programs requiring socketpair-based
local IPC are also unsupported by this initial policy; pipes remain available.
Timeout/cancellation kills the sandbox, so `setsid` does not escape cleanup.

## Practical limits

Networked package installation, Git fetch/push, local TCP test servers and daemon
sockets are intentionally unavailable. Perform those steps yourself outside the
agent. A home-installed rustup/npm toolchain, user package caches and signing keys
are not exposed automatically. Use system/Nix toolchains and project-local,
prepopulated dependency caches for offline builds. There is no per-command grant
UI yet. Commands also cannot stage files in an existing `.git`; stage new files
outside Socium, or use `git_commit` for its supported tracked-file scope.

Workspace contents are still readable by the model through file tools and command
output. `.gitignore` is not a confidentiality boundary. A command may damage any
workspace content its policy permits. Existing hardlinks can alias files outside
the workspace or protected metadata, so use trusted checkouts without such links.
This is not a VM, does not set CPU/memory/disk quotas, and does not defend against
a compromised kernel or hostile processes already running as the host user.
Path-based setup and the parent's file tools are not a race-free boundary against
concurrent host filesystem modification. Cancellation does not undo completed
writes; inspect unknown outcomes before retrying after a crash.

## Verification

Run on a Linux host capable of starting the namespaces (not a container that
prohibits nested namespace creation):

```sh
nix develop --command cargo test --locked --all-targets --all-features
nix develop --command cargo clippy --locked --all-targets --all-features -- -D warnings
```

`crates/core/src/execution/tests.rs` checks allowed workspace writes alongside
denied outside reads/writes, `sed -i`, `env` wrappers, same-name workspace programs,
symlink escapes, hidden state, protected Git metadata, redirected-layout refusal,
fake credential exclusion, fresh HOME/tmp, TCP/UDP/Unix sockets, io_uring,
namespace re-entry, failed backend setup, confined Git hooks and detached-child
timeout cleanup. Existing reliability tests cover approval, cancellation,
restart/resume and preservation of unrelated edits through the same executor.

Design references: [bubblewrap](https://github.com/containers/bubblewrap) and
[Linux seccomp filter documentation](https://docs.kernel.org/userspace-api/seccomp_filter.html).
