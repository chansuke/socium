# SOC-025: symbol検索の対応範囲を明示し必要な言語を評価する

| 項目 | 値 |
|---|---|
| GitHub Issue | [#77](https://github.com/chansuke/socium/issues/77) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p2`、`type:research`、`area:tools`、`status:blocked` |
| GitHub前提 | [SOC-024 / #76](https://github.com/chansuke/socium/issues/76) |
| GitHub後続 | なし |
| 状態 | 未着手 |
| 優先度 | P2 |
| 起票日 | 2026-11-11 |
| 出典 | [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md) — F12 言語対応 |
| 前提チケット | [SOC-024](SOC-024-search-contract-and-limits.md) |
| 作業分類 | 実装・検証 |

現在の実行状態はGitHubのopen/closed・状態ラベル・依存関係を確認する。上のラベルは起票時の記録。[運用規約](github-workflow.md) / [機械可読の対応表](github-issues.json)。

**問題と目的**

find_symbolはRustの宣言らしい行を探すheuristicであり、汎用的なsymbol解決・参照検索ではない。

**対応範囲**

- 現状のheuristicの対応言語と制限をschema・helpで明示する。
- 実タスクで必要な言語と定義/参照の用途を測定し、tree-sitter/LSP導入の要否を決める。
- 採用する場合は言語別backendとfallback・timeoutの契約を定義する。

**受け入れ条件**

- [ ] Rust以外に対応しているかのような名称・説明を出さず、未対応言語を明示する。
- [ ] コメント・文字列・同名symbol・複数行宣言の限界をfixtureで示す。
- [ ] 追加backendを採用するなら従来検索との差を精度・時間で評価する。
- [ ] 導入見送りの場合も用途・根拠・残る制約を文書化し、汎用symbol検索を実装済みとしない。

**根拠・再現条件**

この項目に対応する独立した観察用再現テストは元レポートにない。以下の検証方法で受け入れ条件を確認し、静的指摘と実測結果を区別する。

**検証方法**

- 言語別fixtureとSOC-032のrepo-navigationタスクを使って評価する。

受け入れ条件を満たした結果・実行コマンド・未検証範囲を完了時に記録する。共通の検証方針は[チケット一覧](README.md)を参照。

**主な関連ファイル**

- [crates/core/src/tools/find_symbol.rs](../../crates/core/src/tools/find_symbol.rs)
- [crates/core/src/tools/mod.rs](../../crates/core/src/tools/mod.rs)

**対象外・境界**

全言語LSPサーバーの同梱は必須にしない。

**完了記録**

未実施。実装PR/commit、検証結果、残課題を記入してから状態を更新する。

[一覧へ戻る](README.md)

