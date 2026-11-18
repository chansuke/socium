# SOC-032: 固定taskと外部判定によるagent評価を構築する

| 項目 | 値 |
|---|---|
| GitHub Issue | [#84](https://github.com/chansuke/socium/issues/84) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p2`、`type:testing`、`area:quality`、`status:blocked` |
| GitHub前提 | [SOC-016 / #68](https://github.com/chansuke/socium/issues/68)、[SOC-020 / #72](https://github.com/chansuke/socium/issues/72)、[SOC-030 / #82](https://github.com/chansuke/socium/issues/82)、[SOC-031 / #83](https://github.com/chansuke/socium/issues/83) |
| GitHub後続 | [SOC-033 / #85](https://github.com/chansuke/socium/issues/85)、[SOC-036 / #88](https://github.com/chansuke/socium/issues/88) |
| 状態 | 完了 |
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

2026-11-18 完了（基盤は branch `feat/agent-evaluation`、初回の実モデル評価は下記）。

- task set: 既存 5 件に 16 件を足して 21 件。repo 理解 3、bug fix 4、refactor 3、tests 追加 2、既存変更 3（CRLF、未 commit の変更が残る repository、既存出力を保つ CLI 拡張）、長い依頼 1、長い出力 1、長い会話 2、途中の要件変更 2。複数 turn の task は `-p` の後に `-c -p` で同じ session を続ける。
- 判定: 成功は agent に見せない check（`{task}/check.py` は repository の外）と差分確認の両方で決める。`allowed_changes` の外の変更があれば check が通っても失敗。tests 追加 task は壊した実装（mutant）を検出できるか、regression test は修正前に失敗し修正後に通るかで判定する。
- 記録: task と反復ごとに、成功、check 単独の結果、変更 path と意図しない変更、tool error、provider の再試行、それでも成功したか、承認の拒否（`-p` では人が介入できないので `interventions` は常に 0）、token、費用、未報告 call、時間、round、tool、送った turn、停止理由を JSONL に保存する。
- 再実行: 先頭の manifest に harness commit と未 commit の有無、binary・config・models の hash、model、予算、token 上限、反復数、各 task directory の hash を残す。Socium は sampling seed を送らないので、反復で代える。
- 予算: 実モデルでの実行は `--budget-usd` が必須。models.toml に price のない model や usage を報告しない call があると費用が不明として止める。使った額に最も高かった run の額を足すと予算を超える時点で、次の run を始めない。各 run は `max_tokens` と `timeout` で SIGINT により止めるので、超過は最大 1 run 分。始めなかった run は失敗にも成功にも数えない。
- 比較: `compare.py` は 95% Wilson 区間が重ならない場合だけ一方を優位とし、それ以外は「no clear difference」と件数を出す。task の file が変わった比較は拒否し、片側だけの task は比較外として示す。`controlled`（同じ model・予算・token 上限・設定）と `defaults`（各 harness の既定構成）を manifest で区別し、controlled で条件が違えば比較を拒否する。現時点で接続済みの harness は Socium だけ。
- 検証（mock、費用なし）: `run.py --self-check` で 21 件すべての check が未着手の repository で失敗し、正解例では意図しない変更なしに通ることを確認した。`evals/test_evals.py`（13 件）は実 binary を loopback の `mock_provider.py` に向けて実行し、次を確認する。正解を書くと全 task 成功、何もせず完了と返すと全 task 失敗、余計な file を足すと check が通っても失敗、複数 turn が 1 会話になる、予算で次の run が止まる、token 上限で run が止まる、予算なしの実モデル実行は拒否される、比較の規則。範囲外の変更の判定と token 上限を壊すと該当試験が落ちることも確認した。CI にも追加した。
- 初回の実モデル評価（2026-11-18、harness `2097a89`、debug build）: `ds-flash`（`deepseek-flash`）で 21 task × 3 反復、予算 $5。price は公式料金表（https://api-docs.deepseek.com/quick_start/pricing）のピーク価格（入力 $0.30、出力 $1.20、cache hit $0.006 / 1M token）で計上したので、実際の請求以下。結果は 63/63 成功、意図しない変更 0、provider 再試行 0、承認拒否 0、token 上限・時間切れでの停止 0、未報告 call 0、計上費用 $0.158、1 run の中央値はおおむね 5〜28 秒。tool error は 21 run で計 30 件あったが、すべて成功に回復した（regression-test では修正前に失敗する test を走らせるのが手順に含まれる）。
- 解釈: 全 task が成功したため、この task set はこの model では天井に達しており、変更の良し悪しを区別できない。3 反復では差の判定にも足りない。改善の比較には、より難しい task（複数 file にまたがる修正、曖昧な依頼、長い既存コード）を足す必要がある。
- 難しい task の追加（2026-11-18）: 天井に達したため、`difficulty = "hard"` の task を 9 件足して 30 件にした（仕様からの実装 3、別 module に原因がある bug と複数 bug 2、性能 1、AGENTS.md の生成規則 1、多様な呼び出し元の更新 1、3 turn の会話 1）。`ds-flash`、3 反復、予算 $5 で再評価した結果、basic 63/63（$0.167）、hard 26/27（$0.271）。失敗は `slow-events` の 1 run で、check は通ったが作業用の `_scratch/*.py` を 6 つ残したため範囲外の変更として失敗。hard では tool error 25 件のうち 13 run が回復して成功し、1 run の中央値は 12〜111 秒に伸びた。3 反復なので task 単位の差は判定できない（`compare.py` の規則では hard と basic の差も「no clear difference」の範囲）。この実行は同じ作業ツリーに並行して追加された別の 9 task も含めて 39 task で回したもので、manifest の harness commit `09abb8e` は commit を分け直したため main に残らない。別の 9 task の結果はここに含めない。
- 未実施: 他 harness の runner、他 model での評価、release build での実行。

[一覧へ戻る](README.md)

