# Socium TODO

## Goal

Evolve `socium` from a minimal Anthropic-backed TUI agent into a coding agent that is competitive with Codex and Claude Code, while also supporting a local LLM provider.

## Status

- [ ] Not started
- [x] Done

## Current State

- [x] Only the Anthropic provider is implemented.
- [x] `codex` and `opencode` provider entries exist, but are runtime stubs.
- [x] The agent can only use a small local tool surface:
  - [x] `read_file`
  - [x] `list_files`
  - [x] `search_text`
  - [x] `find_symbol`
  - [x] `run_command`
  - [x] `apply_patch`
  - [x] `git_branch`
  - [x] `git_status`
  - [x] `git_diff`
  - [x] `git_log`
  - [x] `git_commit`
  - [x] text editor with `view`, `str_replace`, `create`, `insert`
- [x] Shell execution, repo navigation, patch editing, and basic git workflows now exist, but there is still no web/docs retrieval or streaming tool progress.
- [x] The TUI is functional but still minimal.

## Priority 0: Core Agent Capability Gaps

These are the highest-value features missing compared with Codex and Claude Code.

### 1. Shell command execution with approvals

- [x] Add a command execution tool for build/test/lint/run workflows.
- [x] Support cwd control, timeout control, stdout/stderr capture, and exit codes.
- [x] Add a permission model for:
  - [x] safe read-only commands
  - [x] workspace-write commands
  - [x] destructive commands requiring explicit approval
- [x] Preserve an execution log in the session transcript.

### 2. Better repo search and navigation

- [x] Add fast text search based on `rg`.
- [x] Add file globbing and filtered file listing.
- [x] Add symbol-oriented navigation where practical.
- [ ] Add line-addressable file references in agent responses.

### 3. Real editing primitives

- [x] Add patch-based editing instead of relying mostly on `str_replace`.
- [ ] Support:
  - [x] delete file
  - [x] rename/move file
  - [x] multi-hunk edits
  - [ ] undo or edit history
  - [x] diff previews before apply where needed
- [x] Keep workspace boundary protections.

### 4. Git-aware workflows

- [x] Add tools for:
  - [x] `git status`
  - [x] `git diff`
  - [x] `git log`
  - [x] branch detection
- [x] Later add safe commit support with explicit user intent.
- [x] Surface dirty worktree warnings before risky edits.

### 5. Streaming and live progress

- [ ] Stream assistant output instead of waiting for the full turn.
- [ ] Show tool invocation progress in the TUI.
- [ ] Show command status updates while tasks are running.

### 6. Context management

- [ ] Replace rough character-based context estimation with model-aware budgeting.
- [ ] Add message summarization/compression for long sessions.
- [ ] Persist summaries separately from raw transcript when needed.

## Priority 1: Provider Architecture

The current architecture is too provider-specific for long-term growth.

### 7. Provider-neutral tool abstraction

- [ ] Decouple core tool definitions from Anthropic-specific tool schemas.
- [ ] Introduce a provider adapter layer:
  - [ ] core tool spec
  - [ ] provider-specific request/response mapping
- [ ] Ensure the same agent loop can work with:
  - [ ] Anthropic
  - [ ] OpenAI/Codex
  - [ ] local LLM backends

### 8. Implement the Codex/OpenAI provider

- [ ] Add provider config loading from env.
- [ ] Implement message sending and tool-calling support.
- [ ] Normalize response blocks into internal `ContentBlock` variants.
- [ ] Verify parity with the agent loop already used by Anthropic.

### 9. Implement the local LLM provider

Planned provider targets may include local OpenAI-compatible servers or direct local runtimes.

- [ ] Define the first supported backend:
  - [ ] OpenAI-compatible local server
  - [ ] Ollama
  - [ ] LM Studio
  - [ ] vLLM
  - [ ] llama.cpp server
- [ ] Prefer an OpenAI-compatible protocol first if possible, because it reduces integration cost.
- [ ] Add capability flags for local providers:
  - [ ] tool calling supported / unsupported
  - [ ] streaming supported / unsupported
  - [ ] context window size
  - [ ] reasoning token support if available
- [ ] Add model selection and endpoint configuration from env.
- [ ] Add graceful fallback behavior when a local model does not support tools.

### 10. Local LLM quality and performance work

- [ ] Add prompt variants tuned for weaker local models.
- [ ] Add smaller-tool mode for local models with limited instruction-following.
- [ ] Add configurable max context, max output, and request timeout.
- [ ] Measure latency, failure rates, and tool-call reliability across providers.

## Priority 2: UX and Productization

### 11. Improve the TUI

- [ ] Show persisted conversation history on startup.
- [ ] Add transcript scrolling.
- [ ] Add tool activity and command output panes.
- [ ] Add `/help` and other documented slash commands.
- [ ] Improve status reporting for:
  - [ ] thinking
  - [ ] running command
  - [ ] waiting for approval
  - [ ] completed
  - [ ] failed

### 12. Add a web/docs retrieval tool

- [ ] Allow the agent to fetch documentation pages and API references.
- [ ] Keep this permissioned and auditable.
- [ ] Distinguish web retrieval from local repo inspection.

### 13. Add task planning support

- [ ] Support explicit plan creation and progress tracking.
- [ ] Let the agent update plan state during long tasks.
- [ ] Expose the plan clearly in the UI.

## Priority 3: Reliability

### 14. End-to-end testing

- [ ] Add integration tests for:
  - [ ] provider request/response handling
  - [ ] tool-use loops
  - [ ] command execution
  - [ ] streaming behavior
- [ ] Add fixture-based tests for multi-turn agent behavior.

### 15. Evaluation harness

- [ ] Create benchmark tasks for:
  - [ ] code search
  - [ ] bug fixing
  - [ ] small refactors
  - [ ] test generation
- [ ] Compare:
  - [ ] Anthropic provider
  - [ ] Codex/OpenAI provider
  - [ ] local LLM provider

### 16. Logging and observability

- [ ] Add structured logs for provider calls, tool invocations, failures, and latency.
- [ ] Make provider/tool debugging easier without exposing secrets.

## Suggested Implementation Order

1. Shell execution with approvals
2. Repo search/navigation tools
3. Patch-based editing and diff support
4. Git integration
5. Streaming responses and live tool progress
6. Provider-neutral tool abstraction
7. Codex/OpenAI provider
8. Local LLM provider
9. Context compression and session management
10. TUI improvements
11. Web/docs retrieval
12. End-to-end evaluation harness

## Milestones

### MVP

Goal: make `socium` a usable coding agent for local repositories.

- [x] Shell command execution with approvals
- [x] Repo search and navigation with `rg`-based tooling
- [x] Patch-based editing and diff support
- [x] Git status and diff integration
- [ ] Basic streaming or live progress feedback in the TUI
- [ ] `/help` and core slash commands
- [ ] Provider-neutral tool abstraction groundwork

Exit criteria:

- [ ] The agent can inspect a repo, edit files, run tests, and explain results in one session.
- [ ] The user can see what tools/commands ran and whether they succeeded.
- [ ] Common coding tasks do not require manual file hunting outside the agent.

### v0.2

Goal: expand provider support and improve reliability for longer coding sessions.

- [ ] Implement the Codex/OpenAI provider
- [ ] Implement the first local LLM provider via an OpenAI-compatible endpoint
- [ ] Add provider capability flags and fallback behavior
- [ ] Improve context budgeting and session summarization
- [ ] Improve TUI transcript handling and command/tool panes
- [ ] Add end-to-end tests for provider and tool loops
- [ ] Add structured logging and observability

Exit criteria:

- [ ] The same agent loop works across Anthropic, OpenAI/Codex, and one local LLM backend.
- [ ] Long sessions remain usable without excessive context loss.
- [ ] Provider failures and tool failures are diagnosable from logs and UI output.

### v0.3

Goal: move from a functional coding agent to a competitive daily-use tool.

- [ ] Add task planning support with visible progress state
- [ ] Expand git-aware workflows, including safer commit support
- [ ] Add web/docs retrieval
- [ ] Improve local LLM prompt/tooling modes for weaker models
- [ ] Add evaluation benchmarks across providers
- [ ] Improve streaming and responsiveness further
- [ ] Tighten permission and safety UX around destructive actions

Exit criteria:

- [ ] The agent can handle multi-step implementation tasks with planning, execution, and validation.
- [ ] Provider comparisons can be measured with repeatable benchmark tasks.
- [ ] Local and remote providers both feel like first-class options instead of experimental paths.

## Notes for Local LLM Provider Planning

- [ ] Do not couple local LLM support to Anthropic-style tool definitions.
- [ ] If local models are weaker, prefer:
  - [ ] narrower tool schemas
  - [ ] shorter prompts
  - [ ] stricter tool argument validation
- [ ] Treat local provider support as a first-class feature, not a fallback hack.
- [ ] The best near-term path is likely:
  1. provider-neutral core abstractions
  2. OpenAI-compatible remote provider
  3. OpenAI-compatible local endpoint
  4. backend-specific local integrations only if needed
