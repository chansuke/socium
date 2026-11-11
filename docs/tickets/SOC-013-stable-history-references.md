# SOC-013: 履歴切詰め後も要約範囲と会話単位を整合させる

| 項目 | 値 |
|---|---|
| GitHub Issue | [#65](https://github.com/chansuke/socium/issues/65) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p2`、`bug`、`area:state`、`area:context`、`status:blocked` |
| GitHub前提 | [SOC-006 / #58](https://github.com/chansuke/socium/issues/58)、[SOC-012 / #64](https://github.com/chansuke/socium/issues/64) |
| GitHub後続 | [SOC-026 / #78](https://github.com/chansuke/socium/issues/78) |
| 状態 | 未着手 |
| 優先度 | P2 |
| 起票日 | 2026-11-11 |
| 出典 | [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md) — F11（履歴参照） |
| 前提チケット | [SOC-006](SOC-006-context-message-contract.md)、[SOC-012](SOC-012-atomic-state-storage.md) |
| 作業分類 | 実装・検証 |

現在の実行状態はGitHubのopen/closed・状態ラベル・依存関係を確認する。上のラベルは起票時の記録。[運用規約](github-workflow.md) / [機械可読の対応表](github-issues.json)。

**問題と目的**

2,000件超の履歴を先頭からdrainしてもsummary.covered_messagesを補正しない。位置依存の参照が別の範囲を指し、tool pairも切れ得る。

**対応範囲**

- summary対象を安定したmessage/event IDまたはsequenceで参照する。
- 起動時と実行時の保持上限、archive対象、context投影を分離する。
- 旧covered_messages形式からの移行と不整合検出を実装する。

**受け入れ条件**

- [ ] 2,000件超・既存summaryありの再起動で、未要約messageを要約済みとして除外しない。
- [ ] 保持境界にtool call/resultがあってもprotocol単位が壊れない。
- [ ] summary範囲の欠落・未来参照・世代不一致を検出して明示する。
- [ ] raw履歴をarchiveしても復旧・監査の参照を維持し、実行時の保持量も制御できる。

**根拠・再現条件**

この項目に対応する独立した観察用再現テストは元レポートにない。以下の検証方法で受け入れ条件を確認し、静的指摘と実測結果を区別する。

**検証方法**

- 2,001件以上、summary範囲が境界の前後にあるfixtureと旧形式migrationを試験する。

受け入れ条件を満たした結果・実行コマンド・未検証範囲を完了時に記録する。共通の検証方針は[チケット一覧](README.md)を参照。

**主な関連ファイル**

- [crates/core/src/lib.rs](../../crates/core/src/lib.rs)
- [crates/core/src/agent/state.rs](../../crates/core/src/agent/state.rs)
- [crates/core/src/agent/context.rs](../../crates/core/src/agent/context.rs)

**対象外・境界**

session fork/treeは含めない。

**完了記録**

未実施。実装PR/commit、検証結果、残課題を記入してから状態を更新する。

[一覧へ戻る](README.md)

