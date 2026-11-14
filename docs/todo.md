# Earlier TODO reconciliation

This replaces the outdated checklist with a mapping of its original task IDs.
The [SOC tickets](tickets/README.md) define acceptance criteria; [GitHub #52](https://github.com/chansuke/socium/issues/52)
tracks live status. Do not maintain a second completion checklist here.

`TODO-*` refers to this document's old numbering. `PLAN-*` in
[improvement-plan.md](improvement-plan.md) is a separate numbering scheme.

| Old ID | Topic | Current behavior and remaining work |
|---|---|---|
| TODO-01 | Commands and approvals | SOC-001 / SOC-004 / SOC-011 implemented. Commands require human approval and Linux OS isolation (SOC-003). Read-only helpers and approved mutations have separate enforced policies; command names do not grant authority. |
| TODO-02 | Search and navigation | SOC-024 implemented with bounded rg search. Symbol search is heuristic; SOC-025 remains. |
| TODO-03 | Editing | SOC-008 / SOC-009 implemented with shared preview, history and conflict-aware undo. File mode restoration remains SOC-040. Multi-file changes are not atomic. |
| TODO-04 | Git | SOC-010 implemented with scoped commits and index preservation. Dirty warnings are informational, not a concurrency lock. |
| TODO-05 | Streaming and progress | SOC-004 / SOC-007 / SOC-011 implemented. Further presentation work belongs to SOC-027. |
| TODO-06 | Context | SOC-006 implemented. Token budgeting remains approximate; SOC-017 / SOC-018 cover budget calibration and summary fidelity. Conversation and summary share one snapshot. |
| TODO-07 | Provider abstraction | Existing Provider trait is useful, but message/tool contracts remain Anthropic-oriented. SOC-014. |
| TODO-08 | OpenAI-compatible provider | Unimplemented; SOC-016 depends on SOC-014 / SOC-015. Nonfunctional provider stubs were removed. |
| TODO-09 | Local models | Unimplemented; start with SOC-016. No local endpoint currently exists. |
| TODO-10 | Local-model quality | Not evaluated; SOC-032 / SOC-033. |
| TODO-11 | TUI and input history | Transcript, commands and persistent input history already exist in `crates/tui/src/`. Width/layout work remains SOC-027 / SOC-028. No repl.rs integration is needed. |
| TODO-12 | Sessions | Same-workspace restart is implemented by SOC-005 / SOC-012; multiple named sessions with `-c`, `-r`, and `/resume` by SOC-026. Stable references remain SOC-013. |
| TODO-13 | Web/docs retrieval | Unimplemented; SOC-037 specifies DeepSeek Search. Fetch is outside that ticket. |
| TODO-14 | Task planning | Prompt/project conventions: SOC-022. Persistent task management/extensions: evaluate under SOC-036. |
| TODO-15 | Integration testing | ScriptedProvider, HTTP fixtures and daily acceptance tests already exist. SOC-038 completed; broader contract fixtures remain SOC-030. |
| TODO-16 | Evaluation harness | Unimplemented; SOC-032 depends on headless execution and observable provider behavior. |
| TODO-17 | Logging and observability | Durable turn events and bounded command artifacts exist. Full attempt/cost accounting remains SOC-020. |

Old MVP/v0.2/v0.3 checklists were speculative and have been retired. A feature is
complete only when its ticket's acceptance criteria have evidence; neither a
stub nor a successful happy-path example establishes a security guarantee.
See [current behavior and recovery](daily-reliability.md).
