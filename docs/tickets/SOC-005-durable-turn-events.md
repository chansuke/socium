# SOC-005: 失敗したturnでも実行済み操作を逐次記録する

| 項目 | 値 |
|---|---|
| GitHub Issue | [#57](https://github.com/chansuke/socium/issues/57) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p1`、`bug`、`area:state`、`area:runtime`、`status:blocked` |
| GitHub前提 | [SOC-012 / #64](https://github.com/chansuke/socium/issues/64) |
| GitHub後続 | [SOC-019 / #71](https://github.com/chansuke/socium/issues/71)、[SOC-020 / #72](https://github.com/chansuke/socium/issues/72)、[SOC-026 / #78](https://github.com/chansuke/socium/issues/78)、[SOC-031 / #83](https://github.com/chansuke/socium/issues/83)、[SOC-035 / #87](https://github.com/chansuke/socium/issues/87) |
| 状態 | 実装・検証済み（マージ状態はGitHubを参照） |
| 優先度 | P1 |
| 起票日 | 2026-11-11 |
| 出典 | [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md) — F5／設計案 SessionStore |
| 前提チケット | [SOC-012](SOC-012-atomic-state-storage.md) |
| 作業分類 | 実装・検証 |

現在の実行状態はGitHubのopen/closed・状態ラベル・依存関係を確認する。上のラベルは起票時の記録。[運用規約](github-workflow.md) / [機械可読の対応表](github-issues.json)。

**問題と目的**

turn成功時だけcontextを本履歴へ戻すため、編集後に次のAPIが失敗すると実ファイルと履歴が一致しなくなる。

**対応範囲**

- turn/step/attempt/toolのIDと、intent・started・finished・failedの記録境界を定義する。
- ユーザー入力と実行前intent、各tool結果をturn全体の成否にかかわらず逐次永続化する。
- 再開時の未終端操作をUnknown等で表し、非冪等操作を自動再実行しない。承認監査を同じ記録へ接続する。

**受け入れ条件**

- [x] 編集成功後の2回目provider失敗でも、同一processと再起動後の双方で実行済み編集を確認できる。
- [x] tool round上限・cancel・tool失敗でも完了済み結果が失われない。
- [x] intentを保存できないときは副作用を開始せず、完了後の保存失敗は実行結果不明を含めて明示する。
- [x] 未終端toolを再開しても二重実行せず、UIとmodel用contextが同じ記録を参照する。

**根拠・再現条件**

- `review_second_request_failure_loses_record_of_completed_edit` — [観察用コード](../review-evidence/2026-11-11/review_repros.rs)

これらは欠陥が存在すると成功する観察用テスト。修正時は期待を安全な挙動へ反転して通常の回帰テストへ移す。元のレビュー証拠は保存する。

**検証方法**

- 各保存境界で故障注入し、restart後の状態と実ファイルを照合する。
- exactly-onceを保証できない外部副作用のケースを明示的にテストする。

受け入れ条件を満たした結果・実行コマンド・未検証範囲を完了時に記録する。共通の検証方針は[チケット一覧](README.md)を参照。

**主な関連ファイル**

- [crates/core/src/lib.rs](../../crates/core/src/lib.rs)
- [crates/core/src/agent/turn.rs](../../crates/core/src/agent/turn.rs)
- [crates/core/src/agent/state.rs](../../crates/core/src/agent/state.rs)
- [crates/core/src/tools/mod.rs](../../crates/core/src/tools/mod.rs)
- [crates/core/src/test_support.rs](../../crates/core/src/test_support.rs)

**対象外・境界**

named sessionとswitch UIはSOC-026。ログformatの物理的破損対策はSOC-012。

**完了記録**

ユーザー入力、assistant intent、承認済み実行開始、各tool結果、turn終了/失敗を同一snapshotへ逐次保存。turn offset・round・attempt・tool ID・event sequenceを記録。第二provider失敗、round上限、cancel、実行前/後の保存失敗と再開時Unknownを検証。

検証: Nix環境のfmt、Clippy（warnings deny）、全125テスト（core 111 / TUI 14）が成功。実API・課金なし。実装と受け入れ試験は本変更セット。関連PRはGitHub Issueのclosing PRを参照。[運用・制約](../daily-reliability.md) / [検証対応表](../daily-reliability-validation.md)。Linuxで検証。macOS実行・OS sandboxは未検証/未実装。

[一覧へ戻る](README.md)

