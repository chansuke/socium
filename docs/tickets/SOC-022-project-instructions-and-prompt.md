# SOC-022: 小さい作業規約とscope付きAGENTS.md読込を実装する

| 項目 | 値 |
|---|---|
| GitHub Issue | [#74](https://github.com/chansuke/socium/issues/74) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p2`、`enhancement`、`area:context`、`area:security`、`status:blocked` |
| GitHub前提 | [SOC-002 / #54](https://github.com/chansuke/socium/issues/54)、[SOC-015 / #67](https://github.com/chansuke/socium/issues/67) |
| GitHub後続 | [SOC-021 / #73](https://github.com/chansuke/socium/issues/73) |
| 状態 | 未着手 |
| 優先度 | P2 |
| 起票日 | 2026-11-11 |
| 出典 | [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md) — F12 Prompt／設計案／dsh agent-instructions比較 |
| 前提チケット | [SOC-002](SOC-002-workspace-path-boundary.md)、[SOC-015](SOC-015-validated-model-config.md) |
| 作業分類 | 実装・検証 |

現在の実行状態はGitHubのopen/closed・状態ラベル・依存関係を確認する。上のラベルは起票時の記録。[運用規約](github-workflow.md) / [機械可読の対応表](github-issues.json)。

**問題と目的**

固定promptは簡潔さ・コード引用中心で、project-specific instructionsの探索・scope・予算・更新を扱わない。

**対応範囲**

- inspect→edit→verify、ユーザー変更の保護、検証事実の正確な報告を小さいprompt sectionに分ける。
- project rootから対象directoryまでのAGENTS.mdを出典・scope・優先順位付きで読み込む。
- byte/token予算、symlink境界、更新/削除/再開時の再読込、引用tool出力との信頼区別を定める。

**受け入れ条件**

- [ ] rootとnestedの指示を対象scopeだけに適用し、競合時の優先順位をfixtureで確認できる。
- [ ] 境界外symlink、過大サイズ、読込失敗を規約に従い扱い、secretや指示を無制限に取り込まない。
- [ ] 指示の更新・削除が後続requestへ反映され、どの内容を送ったか追跡できる。
- [ ] 固定promptが未実施のtest成功や自分での承認を促さず、runtimeの権限制約と矛盾しない。

**根拠・再現条件**

この項目に対応する独立した観察用再現テストは元レポートにない。以下の検証方法で受け入れ条件を確認し、静的指摘と実測結果を区別する。

**検証方法**

- nested scope・同名重複・更新・削除・resume・過大instructionのfixtureを追加する。

受け入れ条件を満たした結果・実行コマンド・未検証範囲を完了時に記録する。共通の検証方針は[チケット一覧](README.md)を参照。

**主な関連ファイル**

- [crates/core/src/constants.rs](../../crates/core/src/constants.rs)
- [crates/core/src/agent/context.rs](../../crates/core/src/agent/context.rs)
- [crates/core/src/lib.rs](../../crates/core/src/lib.rs)
- [crates/core/src/tools/workspace.rs](../../crates/core/src/tools/workspace.rs)

**対象外・境界**

skills高度化・CLAUDE互換・marketplaceはSOC-036で需要を評価してから。promptでF1/F2を代替修正しない。

**完了記録**

2026-11-17 完了（branch `feat/scoped-agents-md`）。

- root の AGENTS.md（なければ CLAUDE.md）を turn ごとに読み直す。変わったときだけ system prompt を作り直し、変更・削除・読めない状態を Activity で一度だけ知らせる。
- 下位ディレクトリのファイルは、そのディレクトリ以下のパスを読む・編集する tool 呼び出しの結果に、外側から順に追記する。root の指示に加わり、その範囲では優先すると明記する。届けたかどうかは文脈中のマーカー（ファイル名と内容の FNV-1a ハッシュ）で判断するので、変更後と圧縮後に再送され、何を送ったかは会話に残る。prompt cache は保たれる。
- workspace 外を指す symlink、32 KiB 超、読込失敗は使わずに警告する。1 回の呼び出しで追記する量も 32 KiB まで。
- 固定 prompt（受け入れ条件 4）: 検証は実施したもの・通ったもの・できなかったものを述べ、拒否された呼び出しは再試行しないと既に指示している。承認を自分で与える経路はない（SOC-001）。
- 試験: `nested_files_cover_their_directory_once_per_version`、`a_nested_file_outside_the_workspace_or_too_large_is_reported`、`tool_inputs_name_the_paths_they_touch`、`instruction_files_follow_edits_and_reach_the_tools_that_use_their_scope`。
- 範囲外: run_command の cwd による適用。コマンドが触るパスは引数から分からないため。

[一覧へ戻る](README.md)

