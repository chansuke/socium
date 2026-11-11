# SOC-026: 複数sessionの作成・再開とtool履歴表示を実装する

| 項目 | 値 |
|---|---|
| GitHub Issue | [#78](https://github.com/chansuke/socium/issues/78) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p2`、`enhancement`、`area:state`、`area:tui`、`status:blocked` |
| GitHub前提 | [SOC-005 / #57](https://github.com/chansuke/socium/issues/57)、[SOC-012 / #64](https://github.com/chansuke/socium/issues/64)、[SOC-013 / #65](https://github.com/chansuke/socium/issues/65) |
| GitHub後続 | なし |
| 状態 | 未着手 |
| 優先度 | P2 |
| 起票日 | 2026-11-11 |
| 出典 | [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md) — F12 Session／pi session比較 |
| 前提チケット | [SOC-005](SOC-005-durable-turn-events.md)、[SOC-012](SOC-012-atomic-state-storage.md)、[SOC-013](SOC-013-stable-history-references.md) |
| 作業分類 | 実装・検証 |

現在の実行状態はGitHubのopen/closed・状態ラベル・依存関係を確認する。上のラベルは起票時の記録。[運用規約](github-workflow.md) / [機械可読の対応表](github-issues.json)。

**問題と目的**

cwdごとに会話1本で、transcript_entriesはtool-only messageを除外する。再起動後に何を実行したかUIから追いにくい。

**対応範囲**

- session ID/nameとnew/continue/list/switchの操作を設け、現在の単一sessionから移行する。
- 永続ログからuser/assistant/tool/approval/failureの表示を再構成する。
- 実行中switch・同時writer・欠落artifact・途中turnの扱いを明確にする。

**受け入れ条件**

- [ ] 同じworkspaceの独立sessionが互いの会話を上書きせず、明示的に再開できる。
- [ ] 再起動後もtool名・結果・失敗と中断状態が確認できる。
- [ ] 旧conversationを消さず移行でき、移行失敗時に戻せる。
- [ ] 実行中switchや競合writerで記録が混線せず、禁止または安全な待機を明示する。

**根拠・再現条件**

この項目に対応する独立した観察用再現テストは元レポートにない。以下の検証方法で受け入れ条件を確認し、静的指摘と実測結果を区別する。

**検証方法**

- 旧形式migration、複数session、途中再開、削除済みartifactのprojectionを試験する。

受け入れ条件を満たした結果・実行コマンド・未検証範囲を完了時に記録する。共通の検証方針は[チケット一覧](README.md)を参照。

**主な関連ファイル**

- [crates/core/src/lib.rs](../../crates/core/src/lib.rs)
- [crates/core/src/agent/state.rs](../../crates/core/src/agent/state.rs)
- [crates/tui/src/app.rs](../../crates/tui/src/app.rs)
- [crates/tui/src/transcript.rs](../../crates/tui/src/transcript.rs)
- [crates/tui/src/main.rs](../../crates/tui/src/main.rs)

**対象外・境界**

session tree/forkは後続需要を確認してから。

**完了記録**

未実施。実装PR/commit、検証結果、残課題を記入してから状態を更新する。

[一覧へ戻る](README.md)

