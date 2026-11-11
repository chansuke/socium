# SOC-011: TUIからの中断とworker終了時の回復を実装する

| 項目 | 値 |
|---|---|
| GitHub Issue | [#63](https://github.com/chansuke/socium/issues/63) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p1`、`bug`、`area:tui`、`area:runtime`、`status:blocked` |
| GitHub前提 | [SOC-004 / #56](https://github.com/chansuke/socium/issues/56) |
| GitHub後続 | [SOC-019 / #71](https://github.com/chansuke/socium/issues/71)、[SOC-031 / #83](https://github.com/chansuke/socium/issues/83)、[SOC-035 / #87](https://github.com/chansuke/socium/issues/87) |
| 状態 | 実装・検証済み（マージ状態はGitHubを参照） |
| 優先度 | P1 |
| 起票日 | 2026-11-11 |
| 出典 | [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md) — F10 |
| 前提チケット | [SOC-004](SOC-004-process-lifecycle.md) |
| 作業分類 | 実装・検証 |

現在の実行状態はGitHubのopen/closed・状態ラベル・依存関係を確認する。上のラベルは起票時の記録。[運用規約](github-workflow.md) / [機械可読の対応表](github-issues.json)。

**問題と目的**

esc to cancelの表示に対しEscはmode変更だけ。workerのoneshot Closedを処理せず、panic時に待機状態から戻れない。

**対応範囲**

- AgentControlのcancelとworker所有権を導入し、HTTP・tool・processへ停止を伝搬する。
- EmptyとClosedを区別し、panic/初期化失敗/送信側終了から回復または明示的に終了する。
- UI終了時のworker/process cleanupと途中結果の扱いを定め、表示を実際の状態に合わせる。

**受け入れ条件**

- [x] Escが進行中HTTPまたはコマンドを中断し、子孫cleanup後に操作可能な状態へ戻る。
- [x] 実行中でもcancelと承認応答は入力でき、通常入力の可否はUIに明示される。
- [x] worker panic・channel Closedで永久待機せず、失敗状態と再開手段を表示する。
- [x] quit後に意図しない子プロセスを残さず、完了済み結果はSOC-005の記録方針に接続できる。

**根拠・再現条件**

この項目に対応する独立した観察用再現テストは元レポートにない。以下の検証方法で受け入れ条件を確認し、静的指摘と実測結果を区別する。

**検証方法**

- fake provider待機・長時間process・worker panicを注入し、制御イベントとUI状態を確認する。

受け入れ条件を満たした結果・実行コマンド・未検証範囲を完了時に記録する。共通の検証方針は[チケット一覧](README.md)を参照。

**主な関連ファイル**

- [crates/tui/src/agent_task.rs](../../crates/tui/src/agent_task.rs)
- [crates/tui/src/app.rs](../../crates/tui/src/app.rs)
- [crates/tui/src/progress.rs](../../crates/tui/src/progress.rs)
- [crates/tui/src/view/text_layout.rs](../../crates/tui/src/view/text_layout.rs)
- [crates/core/src/lib.rs](../../crates/core/src/lib.rs)
- [crates/core/src/agent/turn.rs](../../crates/core/src/agent/turn.rs)

**対象外・境界**

steering/follow-upの通常入力キューはSOC-035。表示修正だけの先行PRは可能だが、このチケット全体の完了とはしない。

**完了記録**

HTTP/承認/command/searchへcancelを伝搬し、Esc/Ctrl+C、worker panic、Closed channel、終了時cleanupを接続。panicで端末が復元されてUIが壊れないようworkerのhookを分離。完了済み結果は保存して再開可能。

検証: Nix環境のfmt、Clippy（warnings deny）、全125テスト（core 111 / TUI 14）が成功。実API・課金なし。実装と受け入れ試験は本変更セット。関連PRはGitHub Issueのclosing PRを参照。[運用・制約](../daily-reliability.md) / [検証対応表](../daily-reliability-validation.md)。Linuxで検証。macOS実行・OS sandboxは未検証/未実装。

[一覧へ戻る](README.md)

