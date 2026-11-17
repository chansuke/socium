# SOC-017: tool roundごとに実測補正付きcontext予算を確認する

| 項目 | 値 |
|---|---|
| GitHub Issue | [#69](https://github.com/chansuke/socium/issues/69) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p2`、`bug`、`area:context`、`status:blocked` |
| GitHub前提 | [SOC-006 / #58](https://github.com/chansuke/socium/issues/58)、[SOC-015 / #67](https://github.com/chansuke/socium/issues/67)、[SOC-023 / #75](https://github.com/chansuke/socium/issues/75) |
| GitHub後続 | [SOC-035 / #87](https://github.com/chansuke/socium/issues/87) |
| 状態 | 未着手 |
| 優先度 | P2 |
| 起票日 | 2026-11-11 |
| 出典 | [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md) — F12 Context予算・turn内増加 |
| 前提チケット | [SOC-006](SOC-006-context-message-contract.md)、[SOC-015](SOC-015-validated-model-config.md)、[SOC-023](SOC-023-structured-tool-output.md) |
| 作業分類 | 実装・検証 |

現在の実行状態はGitHubのopen/closed・状態ラベル・依存関係を確認する。上のラベルは起票時の記録。[運用規約](github-workflow.md) / [機械可読の対応表](github-issues.json)。

**問題と目的**

submit時だけcontextを準備し、turn内で増えるtool結果を再予算化しない。最新usageの表示もその後の入力増加を十分反映しない。

**対応範囲**

- 各provider request直前にsystem/tools/history/output予約を評価する。
- 最新の有効な実測usageと後続message推定を合算し、compact後や設定変更後の古い実測を無効化する。
- pressureとprovider-confirmed overflowを区別し、大出力artifact・縮約・明示エラーを使い分ける。

**受け入れ条件**

- [ ] turn内で大きなtool結果が増えても、予算超過を検出せず次requestへ進まない。
- [ ] pending user inputと前回usage以降の増分がmetricsへ反映される。
- [ ] summary/model/toolsの変更後に古い測定を新contextの実測として表示しない。
- [ ] 縮約後も収まらない場合やoverflow回復失敗では無限retryせず理由を返す。

**根拠・再現条件**

この項目に対応する独立した観察用再現テストは元レポートにない。以下の検証方法で受け入れ条件を確認し、静的指摘と実測結果を区別する。

**検証方法**

- 大出力tool連鎖、再起動後usageなし、model切替、overflow応答のfixtureを追加する。

受け入れ条件を満たした結果・実行コマンド・未検証範囲を完了時に記録する。共通の検証方針は[チケット一覧](README.md)を参照。

**主な関連ファイル**

- [crates/core/src/agent/context.rs](../../crates/core/src/agent/context.rs)
- [crates/core/src/agent/turn.rs](../../crates/core/src/agent/turn.rs)
- [crates/core/src/lib.rs](../../crates/core/src/lib.rs)
- [crates/core/src/agent/compaction.rs](../../crates/core/src/agent/compaction.rs)

**対象外・境界**

全model用の厳密tokenizer実装を必須とせず、推定の誤差と安全余裕を明示する。

**完了記録**

2026-11-17 完了として整理。

- turn ループが round ごとに実測で補正した予算（`ContextBudget::with_scale`）で request を見積もる。超えると直前の tool 結果を縮め（`shrink_tool_results`）、それでも超えれば `ContextFull` で止まり、圧縮してから利用者に続きを委ねる。無限には再試行しない。
- 入力中の文字は `context_metrics_for_input` で metrics に入る。model を変えると補正値を戻す（ADR 15）。

[一覧へ戻る](README.md)

