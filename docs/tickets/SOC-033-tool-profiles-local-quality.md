# SOC-033: tool構成とlocal model向けpromptを評価で選ぶ

| 項目 | 値 |
|---|---|
| GitHub Issue | [#85](https://github.com/chansuke/socium/issues/85) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p2`、`type:research`、`area:quality`、`area:provider`、`status:blocked` |
| GitHub前提 | [SOC-016 / #68](https://github.com/chansuke/socium/issues/68)、[SOC-032 / #84](https://github.com/chansuke/socium/issues/84) |
| GitHub後続 | なし |
| 状態 | 未着手 |
| 優先度 | P2 |
| 起票日 | 2026-11-11 |
| 出典 | [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md) — pi基本tool比較／実装順8 |
| 前提チケット | [SOC-016](SOC-016-openai-compatible-provider.md)、[SOC-032](SOC-032-reproducible-agent-evaluation.md) |
| 作業分類 | 実装・検証 |

現在の実行状態はGitHubのopen/closed・状態ラベル・依存関係を確認する。上のラベルは起票時の記録。[運用規約](github-workflow.md) / [機械可読の対応表](github-issues.json)。

**問題と目的**

ツールを増減するだけで品質が上がるとは言えず、弱いlocal modelでのschema遵守や専用検索toolの効果は未測定。

**対応範囲**

- 現行12tool、少数基本tool、専用検索を加えた構成を設定で切替可能にする。
- 短いprompt/schemaと入力検証の組合せをlocal/remoteの固定taskで比較する。
- 成功率・tool引数失敗・費用・時間の結果に基づき既定構成を決める。

**受け入れ条件**

- [ ] 構成間の差を同じmodel/task/予算で再現できる。
- [ ] 不正tool名・引数への堅牢性を落とさず、非公開toolを実行しない。
- [ ] 既定変更は測定結果とtradeoffを伴い、改善しない場合は現行構成を維持する。
- [ ] local model向け変更がremote modelで回帰しないかを確認する。

**根拠・再現条件**

この項目に対応する独立した観察用再現テストは元レポートにない。以下の検証方法で受け入れ条件を確認し、静的指摘と実測結果を区別する。

**検証方法**

- SOC-032のtask setでtool構成のablationを実施する。

受け入れ条件を満たした結果・実行コマンド・未検証範囲を完了時に記録する。共通の検証方針は[チケット一覧](README.md)を参照。

**主な関連ファイル**

- [crates/core/src/tools/mod.rs](../../crates/core/src/tools/mod.rs)
- [crates/core/src/constants.rs](../../crates/core/src/constants.rs)
- [crates/core/src/agent/context.rs](../../crates/core/src/agent/context.rs)

**対象外・境界**

4toolへの削減を目的や事前結論にしない。

**完了記録**

未実施。実装PR/commit、検証結果、残課題を記入してから状態を更新する。

[一覧へ戻る](README.md)

