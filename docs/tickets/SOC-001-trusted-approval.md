# SOC-001: モデル入力と分離した承認経路を実装する

| 項目 | 値 |
|---|---|
| GitHub Issue | [#53](https://github.com/chansuke/socium/issues/53) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p1`、`bug`、`area:security`、`area:runtime`、`status:ready` |
| GitHub前提 | なし |
| GitHub後続 | [SOC-003 / #55](https://github.com/chansuke/socium/issues/55)、[SOC-010 / #62](https://github.com/chansuke/socium/issues/62)、[SOC-031 / #83](https://github.com/chansuke/socium/issues/83) |
| 状態 | 実装・検証済み（マージ状態はGitHubを参照） |
| 優先度 | P1 |
| 起票日 | 2026-11-11 |
| 出典 | [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md) — F1 |
| 前提チケット | なし |
| 作業分類 | 実装・検証 |

現在の実行状態はGitHubのopen/closed・状態ラベル・依存関係を確認する。上のラベルは起票時の記録。[運用規約](github-workflow.md) / [機械可読の対応表](github-issues.json)。

**問題と目的**

approval文字列やconfirmed=trueをモデルが指定するだけで実行でき、ユーザー承認をruntimeが検証していない。ApprovalRequiredも表示だけで応答経路を持たない。

**対応範囲**

- モデル向けschemaから承認済みの自己申告を取り除き、実行要求とruntime内のgrantを分離する。
- 要求ID・tool call ID・argv・cwd・権限範囲を束ね、TUIからの信頼された応答を受ける。
- 拒否・期限切れ・切断と許可済みscopeの再利用を定義する。監査イベントはSOC-005の永続記録に接続する。

**受け入れ条件**

- [x] モデルがapproval/confirmedを追加しても、許可ポリシーまたは有効なgrantなしでは実行されない。
- [x] 承認した要求と異なるargv/cwd、古い要求ID、重複応答で実行範囲が広がらない。
- [x] 応答経路がない場合、拒否・cancel・timeoutの場合に副作用が発生しない。
- [x] 承認待ちの内容をUIで確認・許可・拒否でき、既に許可されたscopeは規約どおり再利用される。

**根拠・再現条件**

- `review_model_supplied_approval_is_sufficient` — [観察用コード](../review-evidence/2026-11-11/review_repros.rs)

これらは欠陥が存在すると成功する観察用テスト。修正時は期待を安全な挙動へ反転して通常の回帰テストへ移す。元のレビュー証拠は保存する。

**検証方法**

- 偽造・再送・期限切れ・応答切断をfake answererと一時ファイルで検証する。
- git_commitにも同じ承認規約が適用されることを確認する。

受け入れ条件を満たした結果・実行コマンド・未検証範囲を完了時に記録する。共通の検証方針は[チケット一覧](README.md)を参照。

**主な関連ファイル**

- [crates/core/src/tools/run_command.rs](../../crates/core/src/tools/run_command.rs)
- [crates/core/src/tools/git.rs](../../crates/core/src/tools/git.rs)
- [crates/core/src/lib.rs](../../crates/core/src/lib.rs)
- [crates/tui/src/agent_task.rs](../../crates/tui/src/agent_task.rs)
- [crates/tui/src/progress.rs](../../crates/tui/src/progress.rs)

**対象外・境界**

OS sandboxそのものはSOC-003。ユーザーへの毎回の再確認を既定にすることは目的としない。

**完了記録**

モデルの自己申告フィールドを除去し、workspace・tool call・引数に紐付く120秒・一回限りの承認をTUIへ接続。偽造、古いID、重複応答、拒否、期限切れ、中断、応答経路なしを拒否。許可後の実行境界も副作用より前に保存。

検証: Nix環境のfmt、Clippy（warnings deny）、全125テスト（core 111 / TUI 14）が成功。実API・課金なし。実装と受け入れ試験は本変更セット。関連PRはGitHub Issueのclosing PRを参照。[運用・制約](../daily-reliability.md) / [検証対応表](../daily-reliability-validation.md)。Linuxで検証。macOS実行・OS sandboxは未検証/未実装。

[一覧へ戻る](README.md)

