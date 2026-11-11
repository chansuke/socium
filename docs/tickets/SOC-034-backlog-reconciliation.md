# SOC-034: 既存改善計画とTODOを現在の実装・完了基準に合わせる

| 項目 | 値 |
|---|---|
| GitHub Issue | [#86](https://github.com/chansuke/socium/issues/86) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p2`、`documentation`、`area:docs`、`status:ready` |
| GitHub前提 | なし |
| GitHub後続 | なし |
| 状態 | 未着手 |
| 優先度 | P2 |
| 起票日 | 2026-11-11 |
| 出典 | [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md) — 既存の改善計画への批判 |
| 前提チケット | なし |
| 作業分類 | 文書整合 |

現在の実行状態はGitHubのopen/closed・状態ラベル・依存関係を確認する。上のラベルは起票時の記録。[運用規約](github-workflow.md) / [機械可読の対応表](github-issues.json)。

**問題と目的**

旧runtime.rs前提の未解決項目や、実装済み入力履歴の未完了扱いが残る。一方で承認・境界保護・予算は保証不足のまま完了扱いになっている。

**対応範囲**

- 既存task IDを保持して現コード・対応SOCチケットを照合する。
- 実装済み、部分実装、保証未達、未着手を区別し、受け入れ条件でDoneを定義する。
- improvement-planとtodoを同時に整合させ、過去レビューは時点固定の根拠として残す。

**受け入れ条件**

- [ ] 旧path/旧構造を現在の実装の根拠として参照しない。
- [ ] approval/workspace/context/dirty warning/undoの完了状態が実際の保証と一致する。
- [ ] 入力履歴や既存ScriptedProvider試験を未実装として重複計画しない。
- [ ] 既存task ID→SOC IDの対応と、維持・置換・完了の根拠を追跡できる。

**根拠・再現条件**

この項目に対応する独立した観察用再現テストは元レポートにない。以下の検証方法で受け入れ条件を確認し、静的指摘と実測結果を区別する。

**検証方法**

- 関連する2文書の項目対応、コードリンク、SOC ID、statusを相互照合する。

受け入れ条件を満たした結果・実行コマンド・未検証範囲を完了時に記録する。共通の検証方針は[チケット一覧](README.md)を参照。

**主な関連ファイル**

- [docs/improvement-plan.md](../../docs/improvement-plan.md)
- [docs/todo.md](../../docs/todo.md)
- [docs/README.md](../../docs/README.md)

**対象外・境界**

元の詳細レポートと再現証拠は書き換えない。このチケット作成時点では旧計画の状態自体を変更しない。

**完了記録**

未実施。実装PR/commit、検証結果、残課題を記入してから状態を更新する。

[一覧へ戻る](README.md)

