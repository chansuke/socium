# SOC-020: 全attempt・tool・compactionの観測情報を記録する

| 項目 | 値 |
|---|---|
| GitHub Issue | [#72](https://github.com/chansuke/socium/issues/72) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p2`、`enhancement`、`area:runtime`、`area:provider`、`status:blocked` |
| GitHub前提 | [SOC-005 / #57](https://github.com/chansuke/socium/issues/57)、[SOC-015 / #67](https://github.com/chansuke/socium/issues/67) |
| GitHub後続 | [SOC-021 / #73](https://github.com/chansuke/socium/issues/73)、[SOC-032 / #84](https://github.com/chansuke/socium/issues/84) |
| 状態 | 未着手 |
| 優先度 | P2 |
| 起票日 | 2026-11-11 |
| 出典 | [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md) — F12 使用量・コスト／評価方法 |
| 前提チケット | [SOC-005](SOC-005-durable-turn-events.md)、[SOC-015](SOC-015-validated-model-config.md) |
| 作業分類 | 実装・検証 |

現在の実行状態はGitHubのopen/closed・状態ラベル・依存関係を確認する。上のラベルは起票時の記録。[運用規約](github-workflow.md) / [機械可読の対応表](github-issues.json)。

**問題と目的**

最新callのusageのみ保持し、累積・compaction・retry・tool latencyを継続的に比較しにくい。

**対応範囲**

- session/turn/attempt/tool IDをキーにlatency、usage、cache、終了理由を記録する。
- 欠測とzeroを区別し、model価格表の出典/versionと費用推定の有無を明示する。
- credentialを除外した診断出力とTUI/headless共通の観測イベントを提供する。

**受け入れ条件**

- [ ] 通常call・compaction・retryを重複計上せず集計できる。
- [ ] usage未報告を0と偽らず、費用を算出できないmodelは不明と表示する。
- [ ] 失敗のprovider/step/toolと経過時間を追跡できる。
- [ ] API key・認証header・secret環境変数が診断ログに現れない。

**根拠・再現条件**

この項目に対応する独立した観察用再現テストは元レポートにない。以下の検証方法で受け入れ条件を確認し、静的指摘と実測結果を区別する。

**検証方法**

- 複数attempt/compactionの固定usage fixtureとsecret canaryで集計・redactionを検証する。

受け入れ条件を満たした結果・実行コマンド・未検証範囲を完了時に記録する。共通の検証方針は[チケット一覧](README.md)を参照。

**主な関連ファイル**

- [crates/core/src/providers/mod.rs](../../crates/core/src/providers/mod.rs)
- [crates/core/src/agent/turn.rs](../../crates/core/src/agent/turn.rs)
- [crates/core/src/agent/compaction.rs](../../crates/core/src/agent/compaction.rs)
- [crates/core/src/lib.rs](../../crates/core/src/lib.rs)
- [crates/tui/src/progress.rs](../../crates/tui/src/progress.rs)

**対象外・境界**

外部telemetry送信や課金サービス連携は含めない。

**完了記録**

2026-11-17 完了（branch `feat/attempt-observability`）。

- 既に満たしていたもの: model 呼び出しは Activity の `RoundStarted`/`RoundFinished` に turn・round・agent 付きで usage・TTFT・経過時間・終了理由を記録し、activity.jsonl と headless に同じ event が出る。出力前の再試行は失敗した試行ごとに Warning を出し、usage を持たないので重複計上しない。tool は `ToolFinished` に経過時間と失敗を、turn は `TurnFinished` に失敗理由を持つ。usage 未報告の round は「usage n/a」と表示し、価格のない model があれば費用は出さない。API key の header は sensitive 指定で、エラー文は key を出さない。
- 直したもの: 圧縮の LLM 呼び出しの usage を捨てていた（要約を不採用にした場合も課金される）。`CompactionFinished` に応答した model と usage を載せ、通常の round と同じ一つの経路（`SessionUsage::record`）で集計する。自動圧縮・context 超過後の圧縮・手動の `/compact`・サブエージェントの圧縮のすべてで数える。
- 直したもの: usage を報告しない呼び出しが黙って 0 扱いになり、合計と費用が完全な値に見えた。未報告の呼び出しを数え、TUI の footer では合計と費用の前に「≥」を付け、headless の `finished` に `unreported_calls` を出す。
- 試験: `compaction_calls_count_and_unreported_usage_is_not_zero`、`the_summary_call_counts_toward_the_session_usage`、footer の表示試験、要約の不採用時も usage を返す試験。
- 範囲外: 価格表の出典と版の記録。価格は利用者が models.toml に書き、Socium は価格を同梱しない。

[一覧へ戻る](README.md)

