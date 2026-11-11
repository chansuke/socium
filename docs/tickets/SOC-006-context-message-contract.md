# SOC-006: 要約・保持境界をproviderの会話契約に合わせる

| 項目 | 値 |
|---|---|
| GitHub Issue | [#58](https://github.com/chansuke/socium/issues/58) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p1`、`bug`、`area:context`、`area:provider`、`status:ready` |
| GitHub前提 | なし |
| GitHub後続 | [SOC-013 / #65](https://github.com/chansuke/socium/issues/65)、[SOC-017 / #69](https://github.com/chansuke/socium/issues/69)、[SOC-018 / #70](https://github.com/chansuke/socium/issues/70) |
| 状態 | 実装・検証済み（マージ状態はGitHubを参照） |
| 優先度 | P1 |
| 起票日 | 2026-11-11 |
| 出典 | [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md) — F6 |
| 前提チケット | なし |
| 作業分類 | 実装・検証 |

現在の実行状態はGitHubのopen/closed・状態ラベル・依存関係を確認する。上のラベルは起票時の記録。[運用規約](github-workflow.md) / [機械可読の対応表](github-issues.json)。

**問題と目的**

予算内でも最新24件以外を要約し、そのsummaryを先頭system messageにする。別経路の切詰めではtool_resultだけが残る。

**対応範囲**

- 全履歴が予算内かを先に評価し、必要な場合だけ圧縮する。
- normal/emergency/LLM compactionの境界選択を共通化し、tool call/resultとthinking等のprovider契約を保つ。
- summaryを内部表現とprovider wire形式で分け、引用やtool出力をsystem命令へ無条件に昇格させない。

**受け入れ条件**

- [x] 短い25メッセージが予算内なら要約・欠落せず送られる。
- [x] Anthropic向けrequestの先頭に本文付きsystem roleを置かず、対応modelの契約を満たす。
- [x] いずれの圧縮経路でも孤立tool_result、欠落対応ID、分断されたprotocol単位を生成しない。
- [x] 最小保持範囲でも予算超過なら追加縮約または明示エラーとなり、不正なrequestを送らない。

**根拠・再現条件**

- `review_small_history_is_summarized_and_starts_with_system` — [観察用コード](../review-evidence/2026-11-11/review_repros.rs)
- `review_emergency_context_cut_orphans_tool_result` — [観察用コード](../review-evidence/2026-11-11/review_repros.rs)

これらは欠陥が存在すると成功する観察用テスト。修正時は期待を安全な挙動へ反転して通常の回帰テストへ移す。元のレビュー証拠は保存する。

**検証方法**

- raw history→summary→wire requestを一貫して検証するfixtureを追加する。
- この修正をprovider中立化全体の完了待ちにせず、既存Anthropic adapterで先に保証する。

受け入れ条件を満たした結果・実行コマンド・未検証範囲を完了時に記録する。共通の検証方針は[チケット一覧](README.md)を参照。

**主な関連ファイル**

- [crates/core/src/agent/context.rs](../../crates/core/src/agent/context.rs)
- [crates/core/src/agent/compaction.rs](../../crates/core/src/agent/compaction.rs)
- [crates/core/src/agent/types.rs](../../crates/core/src/agent/types.rs)
- [crates/core/src/providers/anthropic/mod.rs](../../crates/core/src/providers/anthropic/mod.rs)

**対象外・境界**

roundごとの予算と実測補正はSOC-017。要約の意味的品質はSOC-018。

**完了記録**

予算に収まる25件はそのまま保持。通常/緊急/明示compactionでtool_result境界を共通化し、summaryをuser dataとして投影。tool ID対応を送信前に検証し、予算超過は明示エラー。

検証: Nix環境のfmt、Clippy（warnings deny）、全125テスト（core 111 / TUI 14）が成功。実API・課金なし。実装と受け入れ試験は本変更セット。関連PRはGitHub Issueのclosing PRを参照。[運用・制約](../daily-reliability.md) / [検証対応表](../daily-reliability-validation.md)。Linuxで検証。macOS実行・OS sandboxは未検証/未実装。

[一覧へ戻る](README.md)

