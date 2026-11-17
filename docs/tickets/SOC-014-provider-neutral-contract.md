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

2026-11-17 完了（branch `refactor/provider-neutral-contract`）。

- 既に満たしていたもの: 同じ turn ループと tool 実行に Anthropic・OpenAI Chat Completions・OpenAI Responses の 3 wire が接続済み（SOC-016、SOC-042）。各 adapter が text・tool 呼び出し・usage・stop reason・thinking を内部の型へ変換し、未対応の終了理由は安全でない停止として扱う。provider 固有の機能は model の features と capabilities で選び、対応しないものは送らずに警告する。
- 直したもの: core の `ToolDefinition` が Anthropic の built-in text editor 型（`text_editor_20250728`）を variant として持ち、`tools_for_model(native)` で model の feature を見て形を選んでいた。core は名前・説明・JSON Schema の一つの形だけを持ち、Anthropic adapter が feature を持つ model への送信時だけ text editor を built-in 型に差し替える。OpenAI adapter は同じ定義を function として送る（以前は built-in 型を拒否していた）。一つの variant だけになった enum は struct にした。
- 試験: Anthropic の built-in model には built-in 型、互換 endpoint と OpenAI の 2 wire には schema のまま送ることを各 adapter の試験で確認。model 切替の前後で core の tool 一覧が変わらないことを確認。既存の stream・context 契約の試験はそのまま通過。
- 範囲外: stop reason の内部語彙は Anthropic 由来の文字列（`tool_use`・`max_tokens` など）のまま。各 adapter が変換しており欠落はない。

[一覧へ戻る](README.md)

