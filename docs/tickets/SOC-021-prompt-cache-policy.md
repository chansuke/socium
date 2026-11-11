# SOC-021: 安定したprompt prefixとcache設定を測定する

| 項目 | 値 |
|---|---|
| GitHub Issue | [#73](https://github.com/chansuke/socium/issues/73) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p2`、`type:research`、`area:provider`、`area:context`、`status:blocked` |
| GitHub前提 | [SOC-014 / #66](https://github.com/chansuke/socium/issues/66)、[SOC-020 / #72](https://github.com/chansuke/socium/issues/72)、[SOC-022 / #74](https://github.com/chansuke/socium/issues/74) |
| GitHub後続 | なし |
| 状態 | 未着手 |
| 優先度 | P2 |
| 起票日 | 2026-11-11 |
| 出典 | [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md) — F12 使用量・コスト／pi・dsh比較 |
| 前提チケット | [SOC-014](SOC-014-provider-neutral-contract.md)、[SOC-020](SOC-020-usage-and-observability.md)、[SOC-022](SOC-022-project-instructions-and-prompt.md) |
| 作業分類 | 実装・検証 |

現在の実行状態はGitHubのopen/closed・状態ラベル・依存関係を確認する。上のラベルは起票時の記録。[運用規約](github-workflow.md) / [機械可読の対応表](github-issues.json)。

**問題と目的**

cache usageを読む一方cache_controlを送らず、prompt構成変更がcache hitへ与える影響を測定していない。

**対応範囲**

- provider対応に応じてcache設定と安定prefixの境界を設計する。
- system/tool/instructionの更新時に誤った再利用をせず、cache非対応providerは明示的に無効化する。
- 同一条件の複数turnでcache有無のtoken/費用/latencyを比較する。

**受け入れ条件**

- [ ] adapterが対応する有効なcache設定だけを送信する。
- [ ] 固定prefixを維持しつつinstruction更新を反映し、古い指示を再利用しない。
- [ ] cache hit/missと測定条件をSOC-020で記録できる。
- [ ] 料金や速度の改善を未測定のまま断定せず、費用比較を再現できる。

**根拠・再現条件**

この項目に対応する独立した観察用再現テストは元レポートにない。以下の検証方法で受け入れ条件を確認し、静的指摘と実測結果を区別する。

**検証方法**

- request snapshotでcache境界を検証し、有料実測は明示実行・予算付きにする。

受け入れ条件を満たした結果・実行コマンド・未検証範囲を完了時に記録する。共通の検証方針は[チケット一覧](README.md)を参照。

**主な関連ファイル**

- [crates/core/src/providers/anthropic/mod.rs](../../crates/core/src/providers/anthropic/mod.rs)
- [crates/core/src/agent/types.rs](../../crates/core/src/agent/types.rs)
- [crates/core/src/agent/context.rs](../../crates/core/src/agent/context.rs)

**対象外・境界**

cacheのためにユーザー要求や安全性を変える最適化は行わない。

**完了記録**

未実施。実装PR/commit、検証結果、残課題を記入してから状態を更新する。

[一覧へ戻る](README.md)

