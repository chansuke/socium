# SOC-032: 固定taskと外部判定によるagent評価を構築する

| 項目 | 値 |
|---|---|
| GitHub Issue | [#84](https://github.com/chansuke/socium/issues/84) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p2`、`type:testing`、`area:quality`、`status:blocked` |
| GitHub前提 | [SOC-016 / #68](https://github.com/chansuke/socium/issues/68)、[SOC-020 / #72](https://github.com/chansuke/socium/issues/72)、[SOC-030 / #82](https://github.com/chansuke/socium/issues/82)、[SOC-031 / #83](https://github.com/chansuke/socium/issues/83) |
| GitHub後続 | [SOC-033 / #85](https://github.com/chansuke/socium/issues/85)、[SOC-036 / #88](https://github.com/chansuke/socium/issues/88) |
| 状態 | 未着手 |
| 優先度 | P2 |
| 起票日 | 2026-11-11 |
| 出典 | [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md) — 評価方法／実装順8 |
| 前提チケット | [SOC-016](SOC-016-openai-compatible-provider.md)、[SOC-020](SOC-020-usage-and-observability.md)、[SOC-030](SOC-030-contract-and-fault-tests.md)、[SOC-031](SOC-031-headless-jsonl.md) |
| 作業分類 | 実装・検証 |

現在の実行状態はGitHubのopen/closed・状態ラベル・依存関係を確認する。上のラベルは起票時の記録。[運用規約](github-workflow.md) / [機械可読の対応表](github-issues.json)。

**問題と目的**

実モデルでの正答率・副作用・費用・介入の比較がなく、設計評価と実測性能を区別した改善判断ができない。

**対応範囲**

- 20〜30件を初期目安にbase commit・task・hidden validator・予算を固定したtask setを用意する。
- repo理解、bug fix、refactor、tests追加、既存変更、長い会話、途中要件変更を含める。
- 同一model/endpoint/予算のharness比較と、各harness既定構成の比較を分けて記録する。

**受け入れ条件**

- [ ] model自己申告ではなく外部testと差分確認で成功を判定する。
- [ ] 成功率、意図しない変更、人間介入、失敗/復旧、token費用、時間をtaskごとに保存する。
- [ ] model・harness commit・設定・入力・seed/反復・validatorを固定して再実行できる。
- [ ] 有料実行は明示的な予算上限を持ち、未実施の比較や不確実な差を性能優位と報告しない。

**根拠・再現条件**

この項目に対応する独立した観察用再現テストは元レポートにない。以下の検証方法で受け入れ条件を確認し、静的指摘と実測結果を区別する。

**検証方法**

- まずmockと小taskでrunner/validatorの正否を確認し、明示した予算内で実モデル評価を行う。

受け入れ条件を満たした結果・実行コマンド・未検証範囲を完了時に記録する。共通の検証方針は[チケット一覧](README.md)を参照。

**主な関連ファイル**

- [docs/review-socium-pi-dsh-2026-11-11.md](../../docs/review-socium-pi-dsh-2026-11-11.md)
- [docs/review-evidence/2026-11-11/run_repros.py](../../docs/review-evidence/2026-11-11/run_repros.py)

**対象外・境界**

SWE-bench等の大規模bench全面対応は初期完了条件に含めない。

**完了記録**

未実施。実装PR/commit、検証結果、残課題を記入してから状態を更新する。

[一覧へ戻る](README.md)

