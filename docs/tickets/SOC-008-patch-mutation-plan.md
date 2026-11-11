# SOC-008: patchの事前検証・preview・適用を同じ計画に統一する

| 項目 | 値 |
|---|---|
| GitHub Issue | [#60](https://github.com/chansuke/socium/issues/60) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p1`、`bug`、`area:tools`、`status:blocked` |
| GitHub前提 | [SOC-002 / #54](https://github.com/chansuke/socium/issues/54) |
| GitHub後続 | [SOC-009 / #61](https://github.com/chansuke/socium/issues/61) |
| 状態 | 実装・検証済み（マージ状態はGitHubを参照） |
| 優先度 | P1 |
| 起票日 | 2026-11-11 |
| 出典 | [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md) — F8 |
| 前提チケット | [SOC-002](SOC-002-workspace-path-boundary.md) |
| 作業分類 | 実装・検証 |

現在の実行状態はGitHubのopen/closed・状態ラベル・依存関係を確認する。上のラベルは起票時の記録。[運用規約](github-workflow.md) / [機械可読の対応表](github-issues.json)。

**問題と目的**

actionを順に適用して後半で失敗すると前半の書込みが残る。dry-runは存在・hunk・移動先を同じ方法で検証しない。

**対応範囲**

- 全actionを事前検証してMutationPlanを作り、dry-runとapplyで共有する。
- 重複hunkの曖昧性、insertion-only/空ファイル、move先競合を明確に扱い、正確なdiffを生成する。
- 適用直前の元内容検査と同一ファイルの競合管理を設け、I/O途中失敗は部分結果と復旧情報を返す。ユーザーの既存変更は実行前に確認する。

**受け入れ条件**

- [x] 存在しないUpdateや不一致hunk、衝突するAdd/Moveはdry-runとapplyの双方で拒否される。
- [x] 事前に判明する後半actionの失敗では、前半にも一切書き込まない。
- [x] 事前検証後の外部変更を検出し、黙って上書きしない。I/O途中失敗時は適用済みpathが分かる。
- [x] 重複・並べ替え・改行を含むdiffが実際の変更と一致し、曖昧なhunkを最初の一致に黙って適用しない。

**根拠・再現条件**

- `review_partial_patch_keeps_first_write_after_second_fails` — [観察用コード](../review-evidence/2026-11-11/review_repros.rs)
- `review_dry_run_accepts_missing_update_and_unchecked_destination` — [観察用コード](../review-evidence/2026-11-11/review_repros.rs)

これらは欠陥が存在すると成功する観察用テスト。修正時は期待を安全な挙動へ反転して通常の回帰テストへ移す。元のレビュー証拠は保存する。

**検証方法**

- multi-file、add/delete/move、同名hunk、BOM/CRLF、disk failureのfixtureを用意する。
- dry-run後に外部編集してからapplyするケースを検証する。

受け入れ条件を満たした結果・実行コマンド・未検証範囲を完了時に記録する。共通の検証方針は[チケット一覧](README.md)を参照。

**主な関連ファイル**

- [crates/core/src/tools/apply_patch.rs](../../crates/core/src/tools/apply_patch.rs)
- [crates/core/src/tools/text_editor.rs](../../crates/core/src/tools/text_editor.rs)
- [crates/core/src/tools/git.rs](../../crates/core/src/tools/git.rs)

**対象外・境界**

複数ファイルの完全atomicityを無条件に約束しない。統一undoはSOC-009。

**完了記録**

dry-run/applyを同一の変更計画へ統一。全path/hunkを先に検証し、曖昧なhunk、move衝突、重複/親子pathを拒否。実行直前の内容照合、完全なbefore/after preview、CRLF、適用済みpathを伴う途中失敗を扱う。

検証: Nix環境のfmt、Clippy（warnings deny）、全125テスト（core 111 / TUI 14）が成功。実API・課金なし。実装と受け入れ試験は本変更セット。関連PRはGitHub Issueのclosing PRを参照。[運用・制約](../daily-reliability.md) / [検証対応表](../daily-reliability-validation.md)。Linuxで検証。macOS実行・OS sandboxは未検証/未実装。

[一覧へ戻る](README.md)

