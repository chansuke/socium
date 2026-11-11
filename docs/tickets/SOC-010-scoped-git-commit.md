# SOC-010: Git commitの対象を明示し既存indexを保護する

| 項目 | 値 |
|---|---|
| GitHub Issue | [#62](https://github.com/chansuke/socium/issues/62) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p1`、`bug`、`area:tools`、`area:security`、`status:blocked` |
| GitHub前提 | [SOC-001 / #53](https://github.com/chansuke/socium/issues/53) |
| GitHub後続 | なし |
| 状態 | 実装・検証済み（マージ状態はGitHubを参照） |
| 優先度 | P1 |
| 起票日 | 2026-11-11 |
| 出典 | [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md) — F9 |
| 前提チケット | [SOC-001](SOC-001-trusted-approval.md) |
| 作業分類 | 実装・検証 |

現在の実行状態はGitHubのopen/closed・状態ラベル・依存関係を確認する。上のラベルは起票時の記録。[運用規約](github-workflow.md) / [機械可読の対応表](github-issues.json)。

**問題と目的**

git_commitが必ずgit add -Aを実行するため、ユーザーの無関係なunstaged/untracked変更をcommitに混入する。

**対応範囲**

- staged-onlyを既定とし、必要なら対象pathを明示する操作を別途定義する。
- commit前に対象diff・既存index・dirty状態を確認し、SOC-001のgrantを正しい範囲に結び付ける。
- stage/commit失敗時のindexと作業ツリーの保持方針を定める。

**受け入れ条件**

- [x] requested.txtのみstageした状態で、unrelated.txtを自動stage/commitしない。
- [x] path指定時も指定外の既存indexとworktreeを変更しない。
- [x] 失敗・拒否・cancelで意図しないstage変更を残さず、復旧が必要なら明示する。
- [x] modelのconfirmed=trueだけではcommitできず、許可された差分範囲と実行が一致する。

**根拠・再現条件**

- `review_git_commit_includes_unrelated_unstaged_files` — [観察用コード](../review-evidence/2026-11-11/review_repros.rs)

これらは欠陥が存在すると成功する観察用テスト。修正時は期待を安全な挙動へ反転して通常の回帰テストへ移す。元のレビュー証拠は保存する。

**検証方法**

- 一時repoで混在したstaged/unstaged/untracked、空commit、commit失敗を試験する。

受け入れ条件を満たした結果・実行コマンド・未検証範囲を完了時に記録する。共通の検証方針は[チケット一覧](README.md)を参照。

**主な関連ファイル**

- [crates/core/src/tools/git.rs](../../crates/core/src/tools/git.rs)

**対象外・境界**

push、PR作成、履歴書換えは含めない。

**完了記録**

staged-onlyを既定にし、任意の明示tracked pathsにも対応。git add -Aを廃止。既存indexとrequested diffを承認表示し、待機中の変化を再検証。混在staged/unstaged/untrackedを保全するテストを追加。

検証: Nix環境のfmt、Clippy（warnings deny）、全125テスト（core 111 / TUI 14）が成功。実API・課金なし。実装と受け入れ試験は本変更セット。関連PRはGitHub Issueのclosing PRを参照。[運用・制約](../daily-reliability.md) / [検証対応表](../daily-reliability-validation.md)。Linuxで検証。macOS実行・OS sandboxは未検証/未実装。

[一覧へ戻る](README.md)

