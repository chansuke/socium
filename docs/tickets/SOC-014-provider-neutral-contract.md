# SOC-014: provider非依存のmessage・tool・response契約を定義する

| 項目 | 値 |
|---|---|
| GitHub Issue | [#66](https://github.com/chansuke/socium/issues/66) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p2`、`type:refactor`、`area:provider`、`status:blocked` |
| GitHub前提 | [SOC-007 / #59](https://github.com/chansuke/socium/issues/59) |
| GitHub後続 | [SOC-016 / #68](https://github.com/chansuke/socium/issues/68)、[SOC-021 / #73](https://github.com/chansuke/socium/issues/73) |
| 状態 | 未着手 |
| 優先度 | P2 |
| 起票日 | 2026-11-11 |
| 出典 | [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md) — F12 Provider／pi・dsh比較 |
| 前提チケット | [SOC-007](SOC-007-stream-completion.md) |
| 作業分類 | 実装・検証 |

現在の実行状態はGitHubのopen/closed・状態ラベル・依存関係を確認する。上のラベルは起票時の記録。[運用規約](github-workflow.md) / [機械可読の対応表](github-issues.json)。

**問題と目的**

Provider traitはあるが、request、Role、ToolDefinitionにAnthropic固有形式が混在する。第二providerへ同じloopを接続しにくい。

**対応範囲**

- 内部message/tool/result、usage、stop reason、provider metadataの境界を定義する。
- Anthropic固有text editor schemaやrole変換をadapterへ移し、coreに別providerのwire形式を持ち込まない。
- provider変換用fixtureと拡張可能な構築経路を用意し、既存AgentService/core-TUI分離を維持する。

**受け入れ条件**

- [ ] coreのtool定義がAnthropic専用schemaを必須とせず、同じtoolを別adapterへ投影できる。
- [ ] text/tool/usage/stop reasonと必要なthinking metadataが変換で欠落しない。
- [ ] 既存Anthropicの正常・異常streamとcontext契約試験が通る。
- [ ] provider固有機能はcapabilitiesと明示エラーで扱い、未対応機能を黙って送信しない。

**根拠・再現条件**

この項目に対応する独立した観察用再現テストは元レポートにない。以下の検証方法で受け入れ条件を確認し、静的指摘と実測結果を区別する。

**検証方法**

- wireのgolden fixtureと内部表現への変換試験を追加し、SOC-016で第二実装へ適用する。

受け入れ条件を満たした結果・実行コマンド・未検証範囲を完了時に記録する。共通の検証方針は[チケット一覧](README.md)を参照。

**主な関連ファイル**

- [crates/core/src/agent/types.rs](../../crates/core/src/agent/types.rs)
- [crates/core/src/providers/mod.rs](../../crates/core/src/providers/mod.rs)
- [crates/core/src/providers/anthropic/mod.rs](../../crates/core/src/providers/anthropic/mod.rs)
- [crates/core/src/tools/mod.rs](../../crates/core/src/tools/mod.rs)
- [crates/core/src/tools/text_editor.rs](../../crates/core/src/tools/text_editor.rs)

**対象外・境界**

新provider実装そのものはSOC-016。既存traitを一から作り直す作業ではない。

**完了記録**

未実施。実装PR/commit、検証結果、残課題を記入してから状態を更新する。

[一覧へ戻る](README.md)

