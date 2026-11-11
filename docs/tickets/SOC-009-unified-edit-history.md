# SOC-009: 全編集ツールの履歴と安全なundoを統一する

| 項目 | 値 |
|---|---|
| GitHub Issue | [#61](https://github.com/chansuke/socium/issues/61) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p2`、`bug`、`area:tools`、`area:state`、`status:blocked` |
| GitHub前提 | [SOC-008 / #60](https://github.com/chansuke/socium/issues/60)、[SOC-012 / #64](https://github.com/chansuke/socium/issues/64) |
| GitHub後続 | なし |
| 状態 | 実装・検証済み（マージ状態はGitHubを参照） |
| 優先度 | P2 |
| 起票日 | 2026-11-11 |
| 出典 | [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md) — F8／既存改善計画のundo記述 |
| 前提チケット | [SOC-008](SOC-008-patch-mutation-plan.md)、[SOC-012](SOC-012-atomic-state-storage.md) |
| 作業分類 | 実装・検証 |

現在の実行状態はGitHubのopen/closed・状態ラベル・依存関係を確認する。上のラベルは起票時の記録。[運用規約](github-workflow.md) / [機械可読の対応表](github-issues.json)。

**問題と目的**

text_editorのみが編集履歴を持ち、apply_patchは参加しない。undoは先に履歴をpopし、後の復元失敗やユーザーの追加編集を十分扱えない。

**対応範囲**

- add/update/delete/moveとtext_editorを共通の変更記録へ接続する。
- 変更前後の内容参照/hashと対象pathを記録し、undo前に現在内容との整合を検査する。
- 復元成功後に履歴状態を更新し、失敗時に履歴を消失させない。保持量と原文artifactの寿命を定める。

**受け入れ条件**

- [x] apply_patchとtext_editor双方の変更を、定義した単位でundoできる。
- [x] ユーザーが後から編集したファイルを警告なしに過去内容へ戻さない。
- [x] undoの書込み失敗時に履歴が残り、再試行または復旧できる。
- [x] delete/move/新規作成のundoもworkspace境界を守り、履歴参照切れを明示する。

**根拠・再現条件**

この項目に対応する独立した観察用再現テストは元レポートにない。以下の検証方法で受け入れ条件を確認し、静的指摘と実測結果を区別する。

**検証方法**

- 編集→外部変更→undo、復元I/O失敗、複数pathの履歴を試験する。

受け入れ条件を満たした結果・実行コマンド・未検証範囲を完了時に記録する。共通の検証方針は[チケット一覧](README.md)を参照。

**主な関連ファイル**

- [crates/core/src/tools/edit_history.rs](../../crates/core/src/tools/edit_history.rs)
- [crates/core/src/tools/text_editor.rs](../../crates/core/src/tools/text_editor.rs)
- [crates/core/src/tools/apply_patch.rs](../../crates/core/src/tools/apply_patch.rs)

**対象外・境界**

任意のshellコマンドやGit commitを完全にundoする機能は含めない。

**完了記録**

patchとtext_editorを同一の変更前後記録へ接続。moveを含む操作単位のundo、外部編集の検出、失敗時に消費しない履歴、未完操作の明示的復旧を実装。旧履歴は保存するが安全なafter情報がないundoは拒否。

検証: Nix環境のfmt、Clippy（warnings deny）、全125テスト（core 111 / TUI 14）が成功。実API・課金なし。実装と受け入れ試験は本変更セット。関連PRはGitHub Issueのclosing PRを参照。[運用・制約](../daily-reliability.md) / [検証対応表](../daily-reliability-validation.md)。Linuxで検証。macOS実行・OS sandboxは未検証/未実装。

[一覧へ戻る](README.md)

