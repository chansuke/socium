# SOC-018: 要約で最新の決定・未解決事項・検証結果を保持する

| 項目 | 値 |
|---|---|
| GitHub Issue | [#70](https://github.com/chansuke/socium/issues/70) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p2`、`bug`、`area:context`、`status:blocked` |
| GitHub前提 | [SOC-006 / #58](https://github.com/chansuke/socium/issues/58) |
| GitHub後続 | なし |
| 状態 | 未着手 |
| 優先度 | P2 |
| 起票日 | 2026-11-11 |
| 出典 | [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md) — F12 要約品質 |
| 前提チケット | [SOC-006](SOC-006-context-message-contract.md) |
| 作業分類 | 実装・検証 |

現在の実行状態はGitHubのopen/closed・状態ラベル・依存関係を確認する。上のラベルは起票時の記録。[運用規約](github-workflow.md) / [機械可読の対応表](github-issues.json)。

**問題と目的**

message先頭600文字やsummary先頭4,000字への切捨てでは、新しい訂正や末尾のエラー、未完作業を失い得る。

**対応範囲**

- 目的・制約・変更path・決定・失敗・検証状態・次工程を保持するsummary構造とmerge規約を定義する。
- 最新訂正を古い要約より優先し、大容量原文は参照可能な記録に残す。
- LLM失敗時の決定論的fallbackと、summary生成がlength終了した場合の扱いを改善する。

**受け入れ条件**

- [ ] 要約を繰り返しても最新の要件訂正と未解決事項が残る。
- [ ] tool出力の末尾エラーや未実施の検証が、成功済みという要約にならない。
- [ ] 保存したraw履歴からsummary対象範囲と根拠を確認できる。
- [ ] fallback時は品質・省略を明示し、空/途中終了summaryを正常な完全要約として扱わない。

**根拠・再現条件**

この項目に対応する独立した観察用再現テストは元レポートにない。以下の検証方法で受け入れ条件を確認し、静的指摘と実測結果を区別する。

**検証方法**

- 長文末尾の制約、訂正の競合、複数回compactの決定論的fixtureを用意する。
- 意味的品質はSOC-032で実モデル評価し、mockだけで品質を証明しない。

受け入れ条件を満たした結果・実行コマンド・未検証範囲を完了時に記録する。共通の検証方針は[チケット一覧](README.md)を参照。

**主な関連ファイル**

- [crates/core/src/agent/compaction.rs](../../crates/core/src/agent/compaction.rs)
- [crates/core/src/agent/context.rs](../../crates/core/src/agent/context.rs)
- [crates/core/src/constants.rs](../../crates/core/src/constants.rs)

**対象外・境界**

恒久的な個人memory/RAG製品の実装は含めない。

**完了記録**

未実施。実装PR/commit、検証結果、残課題を記入してから状態を更新する。

[一覧へ戻る](README.md)

