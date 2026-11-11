# SOC-016: OpenAI互換providerとlocal endpointを実装する

| 項目 | 値 |
|---|---|
| GitHub Issue | [#68](https://github.com/chansuke/socium/issues/68) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p2`、`enhancement`、`area:provider`、`status:blocked` |
| GitHub前提 | [SOC-014 / #66](https://github.com/chansuke/socium/issues/66)、[SOC-015 / #67](https://github.com/chansuke/socium/issues/67) |
| GitHub後続 | [SOC-032 / #84](https://github.com/chansuke/socium/issues/84)、[SOC-033 / #85](https://github.com/chansuke/socium/issues/85) |
| 状態 | 未着手 |
| 優先度 | P2 |
| 起票日 | 2026-11-11 |
| 出典 | [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md) — F12 Provider／実装順7 |
| 前提チケット | [SOC-014](SOC-014-provider-neutral-contract.md)、[SOC-015](SOC-015-validated-model-config.md) |
| 作業分類 | 実装・検証 |

現在の実行状態はGitHubのopen/closed・状態ラベル・依存関係を確認する。上のラベルは起票時の記録。[運用規約](github-workflow.md) / [機械可読の対応表](github-issues.json)。

**問題と目的**

Anthropic以外のproviderはNotImplemented。local modelや第二実providerで共通loopの契約を検証できない。

**対応範囲**

- 最初に対応するOpenAI互換API方式を明示し、設定可能なendpoint/auth/modelで接続する。
- streaming、tool calling、usage、終了理由、HTTPエラーを共通表現へ変換する。
- remote/localの能力差を扱い、Codex製品連携とOpenAI互換APIを設定・名称上で混同しない。

**受け入れ条件**

- [ ] 同じagent fixtureがAnthropicと新adapterでツール往復を完了できる。
- [ ] local mock endpointで認証不要/必要、tool非対応、stream途中失敗を検証できる。
- [ ] tool ID/argumentsのchunk結合、複数tool、usage、length/cancelが正しく変換される。
- [ ] 未対応機能を起動またはrequest前に明示し、無言で別modelへ送らない。

**根拠・再現条件**

この項目に対応する独立した観察用再現テストは元レポートにない。以下の検証方法で受け入れ条件を確認し、静的指摘と実測結果を区別する。

**検証方法**

- HTTP mock fixtureによる契約試験を通常CIへ追加し、実endpoint試験は明示実行とする。

受け入れ条件を満たした結果・実行コマンド・未検証範囲を完了時に記録する。共通の検証方針は[チケット一覧](README.md)を参照。

**主な関連ファイル**

- [crates/core/src/providers/mod.rs](../../crates/core/src/providers/mod.rs)
- [crates/core/src/agent/turn.rs](../../crates/core/src/agent/turn.rs)

**対象外・境界**

全OpenAI互換backend固有機能、Codex OAuth/CLI連携、複数local runtimeの直接統合は含めない。

**完了記録**

未実施。実装PR/commit、検証結果、残課題を記入してから状態を更新する。

[一覧へ戻る](README.md)


公開準備時に未動作のCodex/Opencode雛形を削除した。実装は現行のProvider traitから開始する。
