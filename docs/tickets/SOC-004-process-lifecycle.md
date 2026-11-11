# SOC-004: コマンド出力・タイムアウト・子孫停止を修正する

| 項目 | 値 |
|---|---|
| GitHub Issue | [#56](https://github.com/chansuke/socium/issues/56) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p1`、`bug`、`area:runtime`、`status:ready` |
| GitHub前提 | なし |
| GitHub後続 | [SOC-011 / #63](https://github.com/chansuke/socium/issues/63)、[SOC-023 / #75](https://github.com/chansuke/socium/issues/75) |
| 状態 | 実装・検証済み（マージ状態はGitHubを参照） |
| 優先度 | P1 |
| 起票日 | 2026-11-11 |
| 出典 | [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md) — F4 |
| 前提チケット | なし |
| 作業分類 | 実装・検証 |

現在の実行状態はGitHubのopen/closed・状態ラベル・依存関係を確認する。上のラベルは起票時の記録。[運用規約](github-workflow.md) / [機械可読の対応表](github-issues.json)。

**問題と目的**

終了までstdout/stderrを読まないためpipeが詰まる。子孫がpipeを保持するとtimeoutを超え、signal終了は成功扱いになる。

**対応範囲**

- spawn直後からstdout/stderrを並行して読み、メモリ上限と大容量出力の退避を設ける。
- process group等を使って子孫を含むtimeout/cancelを扱い、drain/cleanupにも期限を設ける。
- 正常終了・非zero・signal・timeout・cancelを構造化し、SOC-011が利用する停止口を用意する。

**受け入れ条件**

- [x] stdout/stderrそれぞれ1 MiB以上、および同時大量出力でも正常コマンドがpipe詰まりでtimeoutしない。
- [x] 親が終了して子孫がpipeを保持するケースでも、timeoutとcleanupの規定時間内に完了する。
- [x] signal終了は成功と区別され、reasonと取得済み出力を返す。
- [x] 出力保持量が設定上限を超えず、途中の出力進捗を通知できる。

**根拠・再現条件**

- `review_pipe_output_causes_false_timeout` — [観察用コード](../review-evidence/2026-11-11/review_repros.rs)
- `review_child_inherited_pipe_outlives_timeout` — [観察用コード](../review-evidence/2026-11-11/review_repros.rs)
- `review_signal_death_reported_as_success` — [観察用コード](../review-evidence/2026-11-11/review_repros.rs)

これらは欠陥が存在すると成功する観察用テスト。修正時は期待を安全な挙動へ反転して通常の回帰テストへ移す。元のレビュー証拠は保存する。

**検証方法**

- wall-clockの厳密一致ではなく許容幅と外側watchdogを使い、ハングでCI全体を止めない。
- 終了後の子孫processとFDの回収を確認する。

受け入れ条件を満たした結果・実行コマンド・未検証範囲を完了時に記録する。共通の検証方針は[チケット一覧](README.md)を参照。

**主な関連ファイル**

- [crates/core/src/tools/run_command.rs](../../crates/core/src/tools/run_command.rs)
- [crates/core/src/lib.rs](../../crates/core/src/lib.rs)

**対象外・境界**

対話的PTYや長期background job機能の追加は含めない。

**完了記録**

stdout/stderrをnonblockingに並行drain。256 KiB/streamのメモリ上限、8 MiB/streamのartifact、進捗通知、Unix process groupの終了と200 msのcleanup上限を実装。両pipe同時2 MiB、親終了後の継承pipe、signal、cancelを検証。

検証: Nix環境のfmt、Clippy（warnings deny）、全125テスト（core 111 / TUI 14）が成功。実API・課金なし。実装と受け入れ試験は本変更セット。関連PRはGitHub Issueのclosing PRを参照。[運用・制約](../daily-reliability.md) / [検証対応表](../daily-reliability-validation.md)。Linuxで検証。macOS実行・OS sandboxは未検証/未実装。

[一覧へ戻る](README.md)

