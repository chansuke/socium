# socium 改善チケット一覧

[2026-11-11の詳細レビュー](../review-socium-pi-dsh-2026-11-11.md)を実施可能な単位に分解したローカルのMarkdownチケット。元レポートと再現証拠を保持し、36件をGitHub Issueとして起票済み。親Issueは [#52](https://github.com/chansuke/socium/issues/52)。

全40件（P1: 12件、P2: 27件、P3: 1件）。元36件に日常信頼性・Web検索の4件を追加した。現在の完了状態はGitHubを参照する。基準となるレビューは `de374125a5ff00cc54b8f94da020250b3ab6b46c` 時点の記録であり、現行コードの未修正事項一覧ではない。

**GitHubでの進め方**

- [進捗・依存関係・着手順 #52](https://github.com/chansuke/socium/issues/52)に40件の子Issueと必須依存を登録済み。
- [ラベルと運用規約](github-workflow.md): 状態遷移、前提と関連参照の区別、完了時の更新手順。
- [機械可読のIssue対応表](github-issues.json): 安定ID、Issue番号、URL、起票時ラベル、依存理由、後続。最新状態はGitHubから取得する。

[日常作業の実装・検証対応表](../daily-reliability-validation.md) / [運用・復旧手順](../daily-reliability.md)。

**使い方と完了条件**

- 現在の実行状態はGitHubのopen/closed・状態ラベル・ネイティブ依存関係を正とする。作業範囲や依存契約を変えたらIssue本文・原票・manifestを一緒に更新する。ローカルの状態とラベルは最終同期時点の記録。
- P1/P2は元レビューの重大度を引き継ぐ。P3は条件付き拡張の採否判断を管理する追加区分であり、原レポートの評価を変更しない。
- 「前提チケット」は受け入れ条件を完了するための依存関係。API設計やfixture作成は先行できる。ID順は厳密な着手順ではない。
- 小さいcore/TUI分離、既存Tool/Provider trait、ScriptedProviderを維持し、修正のために全面framework化しない。
- 一つのチケットが複数PRになる場合も、受け入れ条件をすべて満たすまで完了にしない。共通基盤のticket完了を待たず、各修正に必要な回帰試験を追加する。
- Rust変更時は変更に応じた試験に加え、既存CI相当のfmt・Clippy・testを確認する。文書のみの変更ではリンク・ID・依存関係を検証すればよい。
- 実APIの未実行、OS未検証、費用未測定を明記する。ユーザーのファイルやcredentialを試験fixtureに使わない。

**推奨する着手順**

1. [SOC-001](SOC-001-trusted-approval.md)、[SOC-002](SOC-002-workspace-path-boundary.md)、[SOC-004](SOC-004-process-lifecycle.md)は独立に着手可能。その後[SOC-003](SOC-003-executor-permissions.md)、[SOC-011](SOC-011-cancellation-and-worker-recovery.md)で実効権限と中断を接続する。
2. [SOC-012](SOC-012-atomic-state-storage.md)を[SOC-005](SOC-005-durable-turn-events.md)の前提として早期に進め、[SOC-006](SOC-006-context-message-contract.md)、[SOC-007](SOC-007-stream-completion.md)は独立に会話/stream契約を修正する。[SOC-013](SOC-013-stable-history-references.md)で保存後の履歴参照を整える。
3. [SOC-008](SOC-008-patch-mutation-plan.md)、[SOC-010](SOC-010-scoped-git-commit.md)で編集とcommitの範囲を保護し、[SOC-009](SOC-009-unified-edit-history.md)で共通undoを整備する。
4. [SOC-015](SOC-015-validated-model-config.md)、[SOC-022](SOC-022-project-instructions-and-prompt.md)、[SOC-023](SOC-023-structured-tool-output.md)、[SOC-024](SOC-024-search-contract-and-limits.md)を軸に、[SOC-017](SOC-017-per-request-context-budget.md)、[SOC-018](SOC-018-summary-fidelity.md)、[SOC-019](SOC-019-bounded-provider-retries.md)、[SOC-020](SOC-020-usage-and-observability.md)を各前提の完了に応じて進める。
5. [SOC-014](SOC-014-provider-neutral-contract.md)、[SOC-016](SOC-016-openai-compatible-provider.md)、[SOC-026](SOC-026-named-sessions-and-replay.md)、[SOC-029](SOC-029-runtime-packaging.md)、[SOC-031](SOC-031-headless-jsonl.md)で第二provider、session、配布、自動実行を整える。[SOC-030](SOC-030-contract-and-fault-tests.md)の契約試験基盤は初期修正から育てる。
6. [SOC-021](SOC-021-prompt-cache-policy.md)、[SOC-025](SOC-025-symbol-capability.md)、[SOC-027](SOC-027-tui-layout-and-output.md)、[SOC-028](SOC-028-unicode-terminal-width.md)、[SOC-032](SOC-032-reproducible-agent-evaluation.md)、[SOC-033](SOC-033-tool-profiles-local-quality.md)、[SOC-035](SOC-035-steering-and-follow-up.md)を測定・用途に応じて進める。[SOC-034](SOC-034-backlog-reconciliation.md)の文書整合は独立に実施可能。[SOC-036](SOC-036-extension-adoption-gate.md)の拡張採否は評価後に判断する。

P2の保存基盤などがP1修正の前提になる場合、重大度とは別に先行実施する。本文で限定した範囲を守り、後続拡張を初期修正の必須条件にしない。

**全チケット**

| ID | GitHub | 優先度 | 段階 | 内容 | 前提 |
|---|---|---|---|---|---|
| [SOC-001](SOC-001-trusted-approval.md) | [#53](https://github.com/chansuke/socium/issues/53) | P1 | 権限・パス・process・中断 | モデル入力と分離した承認経路を実装する | なし |
| [SOC-002](SOC-002-workspace-path-boundary.md) | [#54](https://github.com/chansuke/socium/issues/54) | P1 | 権限・パス・process・中断 | 未作成パスを含むworkspace境界検査を修正する | なし |
| [SOC-003](SOC-003-executor-permissions.md) | [#55](https://github.com/chansuke/socium/issues/55) | P1 | 権限・パス・process・中断 | コマンド分類と実効的な実行権限を分離する | [SOC-001](SOC-001-trusted-approval.md)、[SOC-002](SOC-002-workspace-path-boundary.md) |
| [SOC-004](SOC-004-process-lifecycle.md) | [#56](https://github.com/chansuke/socium/issues/56) | P1 | 権限・パス・process・中断 | コマンド出力・タイムアウト・子孫停止を修正する | なし |
| [SOC-005](SOC-005-durable-turn-events.md) | [#57](https://github.com/chansuke/socium/issues/57) | P1 | 記録・stream・context・保存 | 失敗したturnでも実行済み操作を逐次記録する | [SOC-012](SOC-012-atomic-state-storage.md) |
| [SOC-006](SOC-006-context-message-contract.md) | [#58](https://github.com/chansuke/socium/issues/58) | P1 | 記録・stream・context・保存 | 要約・保持境界をproviderの会話契約に合わせる | なし |
| [SOC-007](SOC-007-stream-completion.md) | [#59](https://github.com/chansuke/socium/issues/59) | P1 | 記録・stream・context・保存 | stream終端とモデルの終了理由を検証する | なし |
| [SOC-008](SOC-008-patch-mutation-plan.md) | [#60](https://github.com/chansuke/socium/issues/60) | P1 | 編集・Git | patchの事前検証・preview・適用を同じ計画に統一する | [SOC-002](SOC-002-workspace-path-boundary.md) |
| [SOC-009](SOC-009-unified-edit-history.md) | [#61](https://github.com/chansuke/socium/issues/61) | P2 | 編集・Git | 全編集ツールの履歴と安全なundoを統一する | [SOC-008](SOC-008-patch-mutation-plan.md)、[SOC-012](SOC-012-atomic-state-storage.md) |
| [SOC-010](SOC-010-scoped-git-commit.md) | [#62](https://github.com/chansuke/socium/issues/62) | P1 | 編集・Git | Git commitの対象を明示し既存indexを保護する | [SOC-001](SOC-001-trusted-approval.md) |
| [SOC-011](SOC-011-cancellation-and-worker-recovery.md) | [#63](https://github.com/chansuke/socium/issues/63) | P1 | 権限・パス・process・中断 | TUIからの中断とworker終了時の回復を実装する | [SOC-004](SOC-004-process-lifecycle.md) |
| [SOC-012](SOC-012-atomic-state-storage.md) | [#64](https://github.com/chansuke/socium/issues/64) | P2 | 記録・stream・context・保存 | 状態保存を原子的にしwriter競合と破損から回復する | なし |
| [SOC-013](SOC-013-stable-history-references.md) | [#65](https://github.com/chansuke/socium/issues/65) | P2 | 記録・stream・context・保存 | 履歴切詰め後も要約範囲と会話単位を整合させる | [SOC-006](SOC-006-context-message-contract.md)、[SOC-012](SOC-012-atomic-state-storage.md) |
| [SOC-014](SOC-014-provider-neutral-contract.md) | [#66](https://github.com/chansuke/socium/issues/66) | P2 | provider・session・配布・headless | provider非依存のmessage・tool・response契約を定義する | [SOC-007](SOC-007-stream-completion.md) |
| [SOC-015](SOC-015-validated-model-config.md) | [#67](https://github.com/chansuke/socium/issues/67) | P2 | 設定・context品質・tool・検索 | 起動時設定とmodel capabilitiesを一元化する | なし |
| [SOC-016](SOC-016-openai-compatible-provider.md) | [#68](https://github.com/chansuke/socium/issues/68) | P2 | provider・session・配布・headless | OpenAI互換providerとlocal endpointを実装する | [SOC-014](SOC-014-provider-neutral-contract.md)、[SOC-015](SOC-015-validated-model-config.md) |
| [SOC-017](SOC-017-per-request-context-budget.md) | [#69](https://github.com/chansuke/socium/issues/69) | P2 | 設定・context品質・tool・検索 | tool roundごとに実測補正付きcontext予算を確認する | [SOC-006](SOC-006-context-message-contract.md)、[SOC-015](SOC-015-validated-model-config.md)、[SOC-023](SOC-023-structured-tool-output.md) |
| [SOC-018](SOC-018-summary-fidelity.md) | [#70](https://github.com/chansuke/socium/issues/70) | P2 | 設定・context品質・tool・検索 | 要約で最新の決定・未解決事項・検証結果を保持する | [SOC-006](SOC-006-context-message-contract.md) |
| [SOC-019](SOC-019-bounded-provider-retries.md) | [#71](https://github.com/chansuke/socium/issues/71) | P2 | 設定・context品質・tool・検索 | API障害を分類し安全な範囲でretryする | [SOC-005](SOC-005-durable-turn-events.md)、[SOC-007](SOC-007-stream-completion.md)、[SOC-011](SOC-011-cancellation-and-worker-recovery.md) |
| [SOC-020](SOC-020-usage-and-observability.md) | [#72](https://github.com/chansuke/socium/issues/72) | P2 | 設定・context品質・tool・検索 | 全attempt・tool・compactionの観測情報を記録する | [SOC-005](SOC-005-durable-turn-events.md)、[SOC-015](SOC-015-validated-model-config.md) |
| [SOC-021](SOC-021-prompt-cache-policy.md) | [#73](https://github.com/chansuke/socium/issues/73) | P2 | 評価・UX・用途別改善 | 安定したprompt prefixとcache設定を測定する | [SOC-014](SOC-014-provider-neutral-contract.md)、[SOC-020](SOC-020-usage-and-observability.md)、[SOC-022](SOC-022-project-instructions-and-prompt.md) |
| [SOC-022](SOC-022-project-instructions-and-prompt.md) | [#74](https://github.com/chansuke/socium/issues/74) | P2 | 設定・context品質・tool・検索 | 小さい作業規約とscope付きAGENTS.md読込を実装する | [SOC-002](SOC-002-workspace-path-boundary.md)、[SOC-015](SOC-015-validated-model-config.md) |
| [SOC-023](SOC-023-structured-tool-output.md) | [#75](https://github.com/chansuke/socium/issues/75) | P2 | 設定・context品質・tool・検索 | tool結果を構造化し大容量出力をartifact化する | [SOC-004](SOC-004-process-lifecycle.md) |
| [SOC-024](SOC-024-search-contract-and-limits.md) | [#76](https://github.com/chansuke/socium/issues/76) | P2 | 設定・context品質・tool・検索 | 検索の引数・結果形式・fallback・資源上限を統一する | なし |
| [SOC-025](SOC-025-symbol-capability.md) | [#77](https://github.com/chansuke/socium/issues/77) | P2 | 評価・UX・用途別改善 | symbol検索の対応範囲を明示し必要な言語を評価する | [SOC-024](SOC-024-search-contract-and-limits.md) |
| [SOC-026](SOC-026-named-sessions-and-replay.md) | [#78](https://github.com/chansuke/socium/issues/78) | P2 | provider・session・配布・headless | 複数sessionの作成・再開とtool履歴表示を実装する | [SOC-005](SOC-005-durable-turn-events.md)、[SOC-012](SOC-012-atomic-state-storage.md)、[SOC-013](SOC-013-stable-history-references.md) |
| [SOC-027](SOC-027-tui-layout-and-output.md) | [#79](https://github.com/chansuke/socium/issues/79) | P2 | 評価・UX・用途別改善 | 小画面の会話領域とコマンド失敗表示を改善する | [SOC-015](SOC-015-validated-model-config.md)、[SOC-023](SOC-023-structured-tool-output.md) |
| [SOC-028](SOC-028-unicode-terminal-width.md) | [#80](https://github.com/chansuke/socium/issues/80) | P2 | 評価・UX・用途別改善 | 日本語・結合文字・絵文字の入力と表示幅を修正する | なし |
| [SOC-029](SOC-029-runtime-packaging.md) | [#81](https://github.com/chansuke/socium/issues/81) | P2 | provider・session・配布・headless | 配布物のgit・rg依存と対応環境を明確にする | なし |
| [SOC-030](SOC-030-contract-and-fault-tests.md) | [#82](https://github.com/chansuke/socium/issues/82) | P2 | 記録・stream・context・保存 | wire契約と故障注入の共通テスト基盤を整える | なし |
| [SOC-031](SOC-031-headless-jsonl.md) | [#83](https://github.com/chansuke/socium/issues/83) | P2 | provider・session・配布・headless | 自動検証できるheadless JSONL実行経路を追加する | [SOC-001](SOC-001-trusted-approval.md)、[SOC-005](SOC-005-durable-turn-events.md)、[SOC-007](SOC-007-stream-completion.md)、[SOC-011](SOC-011-cancellation-and-worker-recovery.md)、[SOC-015](SOC-015-validated-model-config.md) |
| [SOC-032](SOC-032-reproducible-agent-evaluation.md) | [#84](https://github.com/chansuke/socium/issues/84) | P2 | 評価・UX・用途別改善 | 固定taskと外部判定によるagent評価を構築する | [SOC-016](SOC-016-openai-compatible-provider.md)、[SOC-020](SOC-020-usage-and-observability.md)、[SOC-030](SOC-030-contract-and-fault-tests.md)、[SOC-031](SOC-031-headless-jsonl.md) |
| [SOC-033](SOC-033-tool-profiles-local-quality.md) | [#85](https://github.com/chansuke/socium/issues/85) | P2 | 評価・UX・用途別改善 | tool構成とlocal model向けpromptを評価で選ぶ | [SOC-016](SOC-016-openai-compatible-provider.md)、[SOC-032](SOC-032-reproducible-agent-evaluation.md) |
| [SOC-034](SOC-034-backlog-reconciliation.md) | [#86](https://github.com/chansuke/socium/issues/86) | P2 | 文書の整合（基盤修正と独立） | 既存改善計画とTODOを現在の実装・完了基準に合わせる | なし |
| [SOC-035](SOC-035-steering-and-follow-up.md) | [#87](https://github.com/chansuke/socium/issues/87) | P2 | 評価・UX・用途別改善 | 実行中の方針修正とfollow-up入力を安全に受け付ける | [SOC-005](SOC-005-durable-turn-events.md)、[SOC-011](SOC-011-cancellation-and-worker-recovery.md)、[SOC-017](SOC-017-per-request-context-budget.md) |
| [SOC-036](SOC-036-extension-adoption-gate.md) | [#88](https://github.com/chansuke/socium/issues/88) | P3 | 需要を確認して判断する拡張 | 追加拡張の需要と費用を評価して採否を決める | [SOC-032](SOC-032-reproducible-agent-evaluation.md) |
| [SOC-037](SOC-037-deepseek-web-search.md) | [#90](https://github.com/chansuke/socium/issues/90) | P2 | 日常信頼性・拡張 | DeepSeek Search APIによるweb_search toolを追加する | なし |
| [SOC-038](SOC-038-daily-reliability-acceptance.md) | [#91](https://github.com/chansuke/socium/issues/91) | P1 | 日常信頼性・拡張 | 日常作業から中断・再起動・再開までの統合回帰シナリオ | #53、#54、#56、#57、#58、#59、#60、#62、#63、#64、#92 |
| [SOC-039](SOC-039-protected-workspace-metadata.md) | [#92](https://github.com/chansuke/socium/issues/92) | P1 | 日常信頼性・拡張 | 通常の編集ツールからGit・セッション状態を保護する | #54、#60 |
| [SOC-040](SOC-040-edit-file-metadata.md) | [#93](https://github.com/chansuke/socium/issues/93) | P2 | 編集・Git | move/delete undoのmetadata保持 | #60、#61 |


**レビューとの対応**

F12は複数領域をまとめた指摘なので、独立して完了判定できるよう分割した。F8/F10/F11も責務が異なる作業を分けた。評価・計画整合・条件付き拡張は、本文後半の提案から起票した。

| レポート項目 | チケット |
|---|---|
| F1 承認 | [SOC-001](SOC-001-trusted-approval.md) |
| F2 新規パス境界 | [SOC-002](SOC-002-workspace-path-boundary.md) |
| F3 実行権限 | [SOC-003](SOC-003-executor-permissions.md) |
| F4 process管理 | [SOC-004](SOC-004-process-lifecycle.md) |
| F5 実行済み操作の記録 | [SOC-005](SOC-005-durable-turn-events.md) |
| F6 会話契約 | [SOC-006](SOC-006-context-message-contract.md) |
| F7 stream終端 | [SOC-007](SOC-007-stream-completion.md) |
| F8 patchとundo | [SOC-008](SOC-008-patch-mutation-plan.md)、[SOC-009](SOC-009-unified-edit-history.md) |
| F9 Git範囲 | [SOC-010](SOC-010-scoped-git-commit.md) |
| F10 中断と操作 | [SOC-011](SOC-011-cancellation-and-worker-recovery.md)、[SOC-035](SOC-035-steering-and-follow-up.md) |
| F11 保存と履歴参照 | [SOC-012](SOC-012-atomic-state-storage.md)、[SOC-013](SOC-013-stable-history-references.md) |
| F12 Provider・capabilities・設定 | [SOC-014](SOC-014-provider-neutral-contract.md)、[SOC-015](SOC-015-validated-model-config.md)、[SOC-016](SOC-016-openai-compatible-provider.md) |
| F12 Context予算・要約 | [SOC-017](SOC-017-per-request-context-budget.md)、[SOC-018](SOC-018-summary-fidelity.md) |
| F12 API障害 | [SOC-019](SOC-019-bounded-provider-retries.md) |
| F12 使用量・コスト・cache | [SOC-020](SOC-020-usage-and-observability.md)、[SOC-021](SOC-021-prompt-cache-policy.md) |
| F12 Prompt | [SOC-022](SOC-022-project-instructions-and-prompt.md) |
| F12 Tool結果 | [SOC-023](SOC-023-structured-tool-output.md) |
| F12 検索・言語対応 | [SOC-024](SOC-024-search-contract-and-limits.md)、[SOC-025](SOC-025-symbol-capability.md) |
| F12 Session | [SOC-026](SOC-026-named-sessions-and-replay.md) |
| F12 UI・文字幅 | [SOC-027](SOC-027-tui-layout-and-output.md)、[SOC-028](SOC-028-unicode-terminal-width.md) |
| F12 配布・品質基盤 | [SOC-029](SOC-029-runtime-packaging.md)、[SOC-030](SOC-030-contract-and-fault-tests.md) |
| 接続形態・評価・tool構成 | [SOC-031](SOC-031-headless-jsonl.md)、[SOC-032](SOC-032-reproducible-agent-evaluation.md)、[SOC-033](SOC-033-tool-profiles-local-quality.md) |
| 既存計画との不整合 | [SOC-034](SOC-034-backlog-reconciliation.md) |
| 条件付きの拡張候補 | [SOC-036](SOC-036-extension-adoption-gate.md) |

**再現証拠と回帰試験**

[観察用テスト](../review-evidence/2026-11-11/review_repros.rs)、[stream観察用テスト](../review-evidence/2026-11-11/stream_repros.rs)、[実行手順](../review-evidence/2026-11-11/run_repros.py)、[結果](../review-evidence/2026-11-11/reproductions.log)を参照。

元の15テストは「欠陥が存在すると成功」する。修正済みのコードでこれらが失敗しても、そのまま回帰とは判定しない。原本は時点固定の証拠として保持し、各修正で期待を安全な挙動に反転した試験を通常のtest suiteへ追加する。元の実行ログにある16 passedには、名前filterに一致した既存previewテスト1件を含む。

| 観察条件 | 担当 |
|---|---|
| `review_model_supplied_approval_is_sufficient` | [SOC-001](SOC-001-trusted-approval.md) |
| `review_new_path_dotdot_escapes_workspace` | [SOC-002](SOC-002-workspace-path-boundary.md) |
| `review_safe_read_only_sed_can_write_without_approval` | [SOC-003](SOC-003-executor-permissions.md) |
| `review_command_reads_outside_workspace` | [SOC-003](SOC-003-executor-permissions.md) |
| `review_pipe_output_causes_false_timeout` | [SOC-004](SOC-004-process-lifecycle.md) |
| `review_child_inherited_pipe_outlives_timeout` | [SOC-004](SOC-004-process-lifecycle.md) |
| `review_signal_death_reported_as_success` | [SOC-004](SOC-004-process-lifecycle.md) |
| `review_second_request_failure_loses_record_of_completed_edit` | [SOC-005](SOC-005-durable-turn-events.md) |
| `review_small_history_is_summarized_and_starts_with_system` | [SOC-006](SOC-006-context-message-contract.md) |
| `review_emergency_context_cut_orphans_tool_result` | [SOC-006](SOC-006-context-message-contract.md) |
| `review_incomplete_stream_can_finish_with_executable_tool` | [SOC-007](SOC-007-stream-completion.md) |
| `review_token_limit_is_discarded_by_stream_builder` | [SOC-007](SOC-007-stream-completion.md) |
| `review_partial_patch_keeps_first_write_after_second_fails` | [SOC-008](SOC-008-patch-mutation-plan.md) |
| `review_dry_run_accepts_missing_update_and_unchecked_destination` | [SOC-008](SOC-008-patch-mutation-plan.md) |
| `review_git_commit_includes_unrelated_unstaged_files` | [SOC-010](SOC-010-scoped-git-commit.md) |

**既存文書との関係**

- [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md): 調査時点の根拠。変更しない。
- この一覧と個別チケット: 今回のレビューに基づく作業範囲・依存・完了条件。
- [既存改善計画](../improvement-plan.md)と[既存TODO](../todo.md): 過去のtask IDと経緯を保つ。現在の保証とずれる記述は[SOC-034](SOC-034-backlog-reconciliation.md)で照合し、両方を一緒に更新する。
- [SOC-036](SOC-036-extension-adoption-gate.md): 実装の約束ではなく採否判断。MCP・skills・plan・subagents・Web UI等は必要性が確認できたものだけ後続起票する。

