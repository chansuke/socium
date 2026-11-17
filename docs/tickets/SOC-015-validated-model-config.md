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

2026-11-17 完了（branch `feat/validated-model-config`）。

- 既に満たしていたもの: model registry（ADR 6）が provider・endpoint・window・出力上限・compaction model・features を一か所で検証し、`context_window` と `max_output_tokens`・`compact_at_tokens` の矛盾、未知の wire、資格情報入りの URL は読込時に拒否する。compaction model が使えなければ警告して自分で要約する。設定は `EnvLookup` で注入され、試験は process の環境変数を書き換えない。TUI の model 表示は起動・`/model`・`/resume` で core の `active_model` から作る。
- 直したもの: `MAX_TOKENS` だけ `from_env` で個別に読み、0 を受け入れ、不正値で起動を止めていた。他の上限と同じく注入された lookup から読み、正の整数でなければ警告して無視する。
- 直したもの: 出力上限（`MAX_TOKENS` か `max_output_tokens`）が window（`CONTEXT_WINDOW_TOKENS` を含む）以上だと入力予算が 1 token になり、どのメッセージも「長すぎる」と拒否され原因が分からなかった。起動時と `/model` で警告し、turn は request 前に理由付きで止める。
- 未知の `MODEL` は Claude の model ID として Anthropic に送る方針を README に明記。
- 試験: `an_output_limit_that_fills_the_window_is_reported_before_any_request`、`no_request_is_sent_when_the_output_limit_fills_the_window`、`switching_to_a_model_whose_output_fills_the_window_says_so`（前二つは修正前に失敗を確認）。
- 範囲外: image・schema 制約などの capability。使う機能がまだない。

[一覧へ戻る](README.md)

