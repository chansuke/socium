# SOC-002: 未作成パスを含むworkspace境界検査を修正する

| 項目 | 値 |
|---|---|
| GitHub Issue | [#54](https://github.com/chansuke/socium/issues/54) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p1`、`bug`、`area:security`、`area:tools`、`status:ready` |
| GitHub前提 | なし |
| GitHub後続 | [SOC-003 / #55](https://github.com/chansuke/socium/issues/55)、[SOC-008 / #60](https://github.com/chansuke/socium/issues/60)、[SOC-022 / #74](https://github.com/chansuke/socium/issues/74) |
| 状態 | 実装・検証済み（マージ状態はGitHubを参照） |
| 優先度 | P1 |
| 起票日 | 2026-11-11 |
| 出典 | [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md) — F2 |
| 前提チケット | なし |
| 作業分類 | 実装・検証 |

現在の実行状態はGitHubのopen/closed・状態ラベル・依存関係を確認する。上のラベルは起票時の記録。[運用規約](github-workflow.md) / [機械可読の対応表](github-issues.json)。

**問題と目的**

存在する祖先だけを検査した後に未正規化のjoinedを返すため、missing/../../escaped.txtでworkspace外へ新規ファイルを作成できる。

**対応範囲**

- 未作成suffixを含めたpath component処理を定義し、既存部分のsymlink解決と合成後の境界を検査する。
- create/add/move/undoなど共通resolverを使う全経路へ修正を適用する。
- 並行symlink差替えを含む脅威モデルと、descriptor基準アクセスまたはsandboxに委ねる範囲を明記する。

**受け入れ条件**

- [x] missing/../../escaped.txtによる境界外への作成を拒否し、workspace外にファイル・ディレクトリを作らない。
- [x] 絶対パス、既存・未作成・dangling symlink、祖先symlink経由の脱出を拒否する。
- [x] workspace内で正当なnested作成と内部のdot/dotdot正規化は契約どおり動く。
- [x] apply_patchとtext_editorのcreate/move/undoで境界の扱いが一致する。

**根拠・再現条件**

- `review_new_path_dotdot_escapes_workspace` — [観察用コード](../review-evidence/2026-11-11/review_repros.rs)

これらは欠陥が存在すると成功する観察用テスト。修正時は期待を安全な挙動へ反転して通常の回帰テストへ移す。元のレビュー証拠は保存する。

**検証方法**

- 一時workspaceと兄弟ディレクトリで成否・副作用の両方を検証する。
- 対応OSのpath形式をfixture化し、未検証OSについて保証を断定しない。

受け入れ条件を満たした結果・実行コマンド・未検証範囲を完了時に記録する。共通の検証方針は[チケット一覧](README.md)を参照。

**主な関連ファイル**

- [crates/core/src/tools/workspace.rs](../../crates/core/src/tools/workspace.rs)
- [crates/core/src/tools/apply_patch.rs](../../crates/core/src/tools/apply_patch.rs)
- [crates/core/src/tools/text_editor.rs](../../crates/core/src/tools/text_editor.rs)

**対象外・境界**

シェルからのファイルアクセス制限はSOC-003。

**完了記録**

未作成pathもcomponent単位で正規化し、symlinkを解決してworkspace境界を検証。missing/../../、dangling symlink、親symlink、絶対pathを副作用前に拒否。並行symlink差替えに対するOS隔離とは区別して文書化。

検証: Nix環境のfmt、Clippy（warnings deny）、全125テスト（core 111 / TUI 14）が成功。実API・課金なし。実装と受け入れ試験は本変更セット。関連PRはGitHub Issueのclosing PRを参照。[運用・制約](../daily-reliability.md) / [検証対応表](../daily-reliability-validation.md)。Linuxで検証。macOS実行・OS sandboxは未検証/未実装。

[一覧へ戻る](README.md)

