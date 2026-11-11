# SOC-036: 追加拡張の需要と費用を評価して採否を決める

| 項目 | 値 |
|---|---|
| GitHub Issue | [#88](https://github.com/chansuke/socium/issues/88) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p3`、`type:research`、`area:quality`、`status:deferred` |
| GitHub前提 | [SOC-032 / #84](https://github.com/chansuke/socium/issues/84) |
| GitHub後続 | なし |
| 状態 | 保留（評価待ち） |
| 優先度 | P3 |
| 起票日 | 2026-11-11 |
| 出典 | [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md) — 実装順9／pi・dsh比較の留保 |
| 前提チケット | [SOC-032](SOC-032-reproducible-agent-evaluation.md) |
| 作業分類 | 調査・採否判断 |

現在の実行状態はGitHubのopen/closed・状態ラベル・依存関係を確認する。上のラベルは起票時の記録。[運用規約](github-workflow.md) / [機械可読の対応表](github-issues.json)。

**問題と目的**

MCP、skills高度化、plan、subagents、Web UI、session forkやreadonly並列化を一括導入すると、基盤修正より先に保守・権限・lifecycle負担が増える。

**対応範囲**

- 各候補の対象task、現状の支障、既存機能で代替できるか、導入/運用費用を整理する。
- 評価task上の効果と、permission・session・cancelへの影響を候補ごとに比較する。
- 採用する候補だけを独立した実装チケットへ切り出し、見送り理由も残す。

**受け入れ条件**

- [ ] 候補ごとに採用/保留/見送りと根拠を記録し、一括実装を完了条件にしない。
- [ ] 採用候補には受け入れ条件・権限境界・中断/再開・運用負担が定義されている。
- [ ] readonly並列化を採用する場合は書込競合と順序の規約が先に存在する。
- [ ] plugin framework/hot reloadの全面導入を、比較対象にあるという理由だけで選ばない。

**根拠・再現条件**

この項目に対応する独立した観察用再現テストは元レポートにない。以下の検証方法で受け入れ条件を確認し、静的指摘と実測結果を区別する。

**検証方法**

- SOC-032の測定結果とユーザーtask上の需要を照合する。

受け入れ条件を満たした結果・実行コマンド・未検証範囲を完了時に記録する。共通の検証方針は[チケット一覧](README.md)を参照。

**主な関連ファイル**

- [docs/review-socium-pi-dsh-2026-11-11.md](../../docs/review-socium-pi-dsh-2026-11-11.md)
- [crates/core/src/tools/mod.rs](../../crates/core/src/tools/mod.rs)
- [crates/core/src/lib.rs](../../crates/core/src/lib.rs)

**対象外・境界**

本チケットは採否判断のみ。各拡張を実装済みにする作業ではない。P3はチケット化時に追加した保留候補の区分で、レビューの重大度を変更するものではない。

**完了記録**

未実施。実装PR/commit、検証結果、残課題を記入してから状態を更新する。

[一覧へ戻る](README.md)

