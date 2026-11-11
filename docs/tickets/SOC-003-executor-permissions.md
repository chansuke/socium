# SOC-003: コマンド分類と実効的な実行権限を分離する

| 項目 | 値 |
|---|---|
| GitHub Issue | [#55](https://github.com/chansuke/socium/issues/55) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p1`、`bug`、`area:security`、`area:runtime`、`status:blocked` |
| GitHub前提 | [SOC-001 / #53](https://github.com/chansuke/socium/issues/53)、[SOC-002 / #54](https://github.com/chansuke/socium/issues/54) |
| GitHub後続 | なし |
| 状態 | 未着手 |
| 優先度 | P1 |
| 起票日 | 2026-11-11 |
| 出典 | [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md) — F3／設計案 Executor |
| 前提チケット | [SOC-001](SOC-001-trusted-approval.md)、[SOC-002](SOC-002-workspace-path-boundary.md) |
| 作業分類 | 実装・検証 |

現在の実行状態はGitHubのopen/closed・状態ラベル・依存関係を確認する。上のラベルは起票時の記録。[運用規約](github-workflow.md) / [機械可読の対応表](github-issues.json)。

**問題と目的**

sed/env等の名前によるread-only分類やcwd制限は、書込み・workspace外読取・network・credentialへのアクセスを拘束しない。

**対応範囲**

- read/write/network範囲を明示するExecutionPolicyと、実際に強制するExecutorを分ける。
- 対応OSとsandbox backendを選定し、未対応環境では隔離を装わず拒否または明示的なunconfinedモードにする。
- 実行ファイルのidentity、argv、environmentを扱い、provider credentialと子プロセス環境を分離する。

**受け入れ条件**

- [ ] restrictedモードでsed -i、env経由の実行、同名別実行ファイルでも許可外書込みを拒否する。
- [ ] policyで許可しないworkspace外read/writeとnetworkアクセスをbackendが強制的に拒否する。
- [ ] provider credentialが既定で子プロセスへ引き継がれず、必要な環境変数は許可規約で渡す。
- [ ] sandbox初期化失敗時に無言でunconfinedへ降格しない。UI・READMEが実際の保証を説明する。

**根拠・再現条件**

- `review_safe_read_only_sed_can_write_without_approval` — [観察用コード](../review-evidence/2026-11-11/review_repros.rs)
- `review_command_reads_outside_workspace` — [観察用コード](../review-evidence/2026-11-11/review_repros.rs)

これらは欠陥が存在すると成功する観察用テスト。修正時は期待を安全な挙動へ反転して通常の回帰テストへ移す。元のレビュー証拠は保存する。

**検証方法**

- 一時領域と非機密のcanary環境変数だけで拒否・許可を試験する。
- OSごとのenforcement試験を実施し、未対応範囲を文書化する。

受け入れ条件を満たした結果・実行コマンド・未検証範囲を完了時に記録する。共通の検証方針は[チケット一覧](README.md)を参照。

**主な関連ファイル**

- [crates/core/src/tools/run_command.rs](../../crates/core/src/tools/run_command.rs)
- [crates/core/src/tools/mod.rs](../../crates/core/src/tools/mod.rs)
- [crates/core/src/tools/workspace.rs](../../crates/core/src/tools/workspace.rs)
- [crates/core/src/providers/anthropic/mod.rs](../../crates/core/src/providers/anthropic/mod.rs)
- [SECURITY.md](../../SECURITY.md)

**対象外・境界**

コマンド名allowlistの拡大をsandboxの代替にしない。dsh相当のplugin frameworkは導入しない。

**完了記録**

未実施。実装PR/commit、検証結果、残課題を記入してから状態を更新する。

[一覧へ戻る](README.md)

