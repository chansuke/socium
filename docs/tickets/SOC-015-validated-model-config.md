# SOC-015: 起動時設定とmodel capabilitiesを一元化する

| 項目 | 値 |
|---|---|
| GitHub Issue | [#67](https://github.com/chansuke/socium/issues/67) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p2`、`type:refactor`、`area:provider`、`status:ready` |
| GitHub前提 | なし |
| GitHub後続 | [SOC-016 / #68](https://github.com/chansuke/socium/issues/68)、[SOC-017 / #69](https://github.com/chansuke/socium/issues/69)、[SOC-020 / #72](https://github.com/chansuke/socium/issues/72)、[SOC-022 / #74](https://github.com/chansuke/socium/issues/74)、[SOC-027 / #79](https://github.com/chansuke/socium/issues/79)、[SOC-031 / #83](https://github.com/chansuke/socium/issues/83) |
| 状態 | 未着手 |
| 優先度 | P2 |
| 起票日 | 2026-11-11 |
| 出典 | [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md) — F12 能力情報・設定 |
| 前提チケット | なし |
| 作業分類 | 実装・検証 |

現在の実行状態はGitHubのopen/closed・状態ラベル・依存関係を確認する。上のラベルは起票時の記録。[運用規約](github-workflow.md) / [機械可読の対応表](github-issues.json)。

**問題と目的**

MODEL、compaction model、endpoint、timeout等が固定値や分散したenv読み込みに依存し、capabilitiesはsupports_toolsのみ。UIのmodel表示も実値とずれ得る。

**対応範囲**

- 検証済みAgentConfig/ModelInfoにprovider、endpoint、model、context/output、timeout、reasoning、compaction設定を集約する。
- 設定sourceの優先順位と未知modelの扱いを決め、互換性のない値を起動時に診断する。
- tools/streaming/reasoning/image/schema制約等を表現し、coreとTUIが同じ解決済み設定を読む。

**受け入れ条件**

- [ ] 不正な数値やcontext/outputの矛盾、未実装providerは最初の有料request前に診断される。
- [ ] compaction modelを対象providerから独立に誤選択せず、未設定時の方針が明示される。
- [ ] UIのmodel/endpoint能力表示が実際のrequest設定と一致する。
- [ ] テストがprocess-global envの書換えに依存せず設定を注入できる。

**根拠・再現条件**

この項目に対応する独立した観察用再現テストは元レポートにない。以下の検証方法で受け入れ条件を確認し、静的指摘と実測結果を区別する。

**検証方法**

- env/default/明示設定の優先順位、未知model、非対応thinkingのtable-driven試験を追加する。

受け入れ条件を満たした結果・実行コマンド・未検証範囲を完了時に記録する。共通の検証方針は[チケット一覧](README.md)を参照。

**主な関連ファイル**

- [crates/core/src/lib.rs](../../crates/core/src/lib.rs)
- [crates/core/src/constants.rs](../../crates/core/src/constants.rs)
- [crates/core/src/providers/mod.rs](../../crates/core/src/providers/mod.rs)
- [crates/core/src/providers/anthropic/mod.rs](../../crates/core/src/providers/anthropic/mod.rs)
- [crates/core/src/agent/compaction.rs](../../crates/core/src/agent/compaction.rs)
- [crates/tui/src/app.rs](../../crates/tui/src/app.rs)

**対象外・境界**

全model catalogの自動同期は必須にしない。

**完了記録**

未実施。実装PR/commit、検証結果、残課題を記入してから状態を更新する。

[一覧へ戻る](README.md)

