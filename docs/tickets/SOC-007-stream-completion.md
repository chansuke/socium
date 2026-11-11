# SOC-007: stream終端とモデルの終了理由を検証する

| 項目 | 値 |
|---|---|
| GitHub Issue | [#59](https://github.com/chansuke/socium/issues/59) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p1`、`bug`、`area:provider`、`area:runtime`、`status:ready` |
| GitHub前提 | なし |
| GitHub後続 | [SOC-014 / #66](https://github.com/chansuke/socium/issues/66)、[SOC-019 / #71](https://github.com/chansuke/socium/issues/71)、[SOC-031 / #83](https://github.com/chansuke/socium/issues/83) |
| 状態 | 実装・検証済み（マージ状態はGitHubを参照） |
| 優先度 | P1 |
| 起票日 | 2026-11-11 |
| 出典 | [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md) — F7 |
| 前提チケット | なし |
| 作業分類 | 実装・検証 |

現在の実行状態はGitHubのopen/closed・状態ラベル・依存関係を確認する。上のラベルは起票時の記録。[運用規約](github-workflow.md) / [機械可読の対応表](github-issues.json)。

**問題と目的**

message_stopとcontent_block_stopを状態として扱わず、stop_reasonも捨てるため、未完streamやmax_tokens終了を通常の結果として返す。

**対応範囲**

- stream/blockの開始・完了とtool引数の完成を検証し、不正な遷移やEOFを分類する。
- ProviderResponseに終了理由を保持し、agent loopが正常完了・tool use・length・refusal・cancel・errorを区別する。
- partial表示と確定メッセージを分け、未完・length終了からtoolを実行しない。

**受け入れ条件**

- [x] message_stop未受信のEOFは通常成功にならず、保持されているtool JSONも実行されない。
- [x] max_tokens理由が呼出元まで残り、継続または停止の方針が通常end_turnと区別される。
- [x] 完了した正常streamのtext/thinking/tool/usageは従来どおり復元できる。
- [x] chunk分割・途中error・不正index等で部分出力と失敗理由が矛盾しない。

**根拠・再現条件**

- `review_incomplete_stream_can_finish_with_executable_tool` — [観察用コード](../review-evidence/2026-11-11/stream_repros.rs)
- `review_token_limit_is_discarded_by_stream_builder` — [観察用コード](../review-evidence/2026-11-11/stream_repros.rs)

これらは欠陥が存在すると成功する観察用テスト。修正時は期待を安全な挙動へ反転して通常の回帰テストへ移す。元のレビュー証拠は保存する。

**検証方法**

- SSE fixtureを細分化したchunkと正常EOF/異常EOFの双方で再生する。
- loopまで通す試験で副作用を持つfake toolの未呼出しを検証する。

受け入れ条件を満たした結果・実行コマンド・未検証範囲を完了時に記録する。共通の検証方針は[チケット一覧](README.md)を参照。

**主な関連ファイル**

- [crates/core/src/providers/anthropic/stream.rs](../../crates/core/src/providers/anthropic/stream.rs)
- [crates/core/src/providers/anthropic/sse.rs](../../crates/core/src/providers/anthropic/sse.rs)
- [crates/core/src/providers/anthropic/mod.rs](../../crates/core/src/providers/anthropic/mod.rs)
- [crates/core/src/providers/mod.rs](../../crates/core/src/providers/mod.rs)
- [crates/core/src/agent/turn.rs](../../crates/core/src/agent/turn.rs)

**対象外・境界**

retry/backoffはSOC-019。

**完了記録**

message_start/stop、block index/type/stop、stop_reasonを検証。不完全EOF、max_tokens、未知/重複sequenceからtoolを実行しない。SSE/response byte上限とtext/thinking/tool/usageの復元テストを追加。

検証: Nix環境のfmt、Clippy（warnings deny）、全125テスト（core 111 / TUI 14）が成功。実API・課金なし。実装と受け入れ試験は本変更セット。関連PRはGitHub Issueのclosing PRを参照。[運用・制約](../daily-reliability.md) / [検証対応表](../daily-reliability-validation.md)。Linuxで検証。macOS実行・OS sandboxは未検証/未実装。

[一覧へ戻る](README.md)

