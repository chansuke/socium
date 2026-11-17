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

- 最初に対応するOpenAI互換API方式を明示し、設定可能なendpoint/auth/modelで接続する。[ADR 6](../adr/0006-モデルを接続形式と接続先の組で登録しmodelコマンドで切り替える.md)に従い、Chat Completionsを `wire = "openai-chat"` としてSOC-041のmodel registryへ追加し、OpenAI本家のGPT-5を最初の接続先とする。Ollama Cloud・OpenRouterは同じwireの別endpointとして扱う。
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

2026-11-13 実装中（branch `feat/openai-chat-wire`、#108 の上に積む）。

- `wire = "openai-chat"`（`crates/core/src/providers/openai_chat/`）。`{base_url}/chat/completions` へ Bearer 認証・stream・`stream_options.include_usage`・`max_completion_tokens` で送る。`effort` feature で `EFFORT` を `reasoning_effort` として送る。
- 履歴の変換は adapter 内で行い、core の型は変えていない（SOC-014 の共通契約は SOC-042 で必要になった時点で定める）。thinking は送らず、tool 結果は `tool` メッセージ、エラー結果は `Error: ` を前置する。
- stream: tool call 断片の index ごとの結合、index なしの一括 tool call、`stop` で終わる tool call（互換 server）、`reasoning_content`/`reasoning` の Activity 表示（履歴には残さない）、usage（cached を cache read として）、`length`・`content_filter`・finish_reason 欠落・stream 中の error・不正な arguments を検証。
- 試験: 単体試験と loopback の mock HTTP server（認証ヘッダー、tool call、401 の表示、finish_reason 前の切断）。core 212、tui 69 通過。
- 未検証: OpenAI 本家（キーなし）。OpenAI の現行一覧では GPT-5 系の汎用モデルが外れ GPT-6 系が主力のため、設定例は `gpt-6-astra`。認証不要の local endpoint（キーの環境変数は必須のまま）、tool 非対応 model の扱い、複数 round の実 API 試験。

[一覧へ戻る](README.md)


公開準備時に未動作のCodex/Opencode雛形を削除した。実装は現行のProvider traitから開始する。

2026-11-17 残りを実装。

- 認証不要の local endpoint: localhost の `base_url` なら `api_key_env` を省略でき、そのとき key header を送らない。remote で省略したもの、key なしの `auth` は読込時に拒否する（`feat/keyless-local-endpoint`）。
- tool 非対応 model: models.toml の `tools = false` で、tool 定義を送らず 1 回の応答で終える。起動時と `/model` 切替時に警告し、無言で別 model へ送らない（`feat/models-without-tools`）。
- 試験: `a_provider_on_this_machine_may_go_without_a_key`、`a_local_endpoint_without_a_key_gets_no_authorization_header`、`a_model_without_tools_is_sent_none_and_answers_in_one_round`、`switching_to_a_model_without_tools_says_so`。
- 未検証: 実際の Ollama・llama.cpp・tool 非対応 model への送信。
