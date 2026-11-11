# GitHub Issueのラベル・依存関係・運用

[チケット一覧](README.md) / [親Issue #52](https://github.com/chansuke/socium/issues/52) / [JSON対応表](github-issues.json)

2026-11-11のレビュー由来36件と追加4件を、親Issueに集約した。SOC IDは原票とGitHubをまたぐ安定ID。Issue番号はGitHub側の識別子で、同じ数字ではない。実行状態はGitHubを正とし、原票・JSONの状態は起票時のスナップショットとして扱う。

## ラベル設計

優先度を1つ、作業種別を1つ、状態を1つ、領域を1つ以上付ける。元レビュー由来の36件にレビュー集合ラベルを付ける。追加チケットには由来を誤認させないラベルを付ける。親Issueは集合ラベルと `type:tracking` のみ。既存の `bug`・`enhancement`・`documentation` を再利用し、他の既存ラベルは変更していない。

| 作業種別 | 意味 |
|---|---|
| `bug` | 現在の動作・保証・品質上の欠陥を修正する |
| `enhancement` | 新しい機能・運用経路を追加する |
| `documentation` | 計画・TODO・実装の整合性を直す |
| `type:refactor` | 既存の責務・内部契約を整理する |
| `type:research` | 測定や採否判断を成果物とする |
| `type:testing` | 検証・評価基盤を整える |

| 追加ラベル | 意味 |
|---|---|
| `review:2026-11-11` | 2026-11-11 socium/pi/dshレビュー由来。SOC IDで追跡 |
| `priority:p1` | P1: 権限・整合性・主要動作の重大な欠陥。前提基盤も先行する |
| `priority:p2` | P2: 信頼性・品質・機能の改善。P1の前提なら先行する |
| `priority:p3` | P3: 評価後に採否を決める条件付き拡張 |
| `status:ready` | 未着手・未完了の必須前提なし。設計上の関連Issueは別扱い |
| `status:blocked` | 未完了の必須前提あり。設計/fixtureは先行可、完了は待つ |
| `status:deferred` | 意図的に保留。前提完了に加えて採否・着手判断が必要 |
| `status:in-progress` | 担当者が着手済み。完了時に状態ラベルを外してclose |
| `type:refactor` | 責務・内部契約・設定構造の整理 |
| `type:research` | 測定・選択・採否判断が成果物。一括実装を約束しない |
| `type:testing` | 契約・故障注入・外部評価の検証基盤 |
| `type:tracking` | 複数Issueの進捗・依存関係を集約する親Issue |
| `area:security` | 承認・権限・workspace境界 |
| `area:runtime` | turn・実行process・中断・retry・観測 |
| `area:state` | 永続化・履歴・session・復旧 |
| `area:context` | context予算・要約・指示読込 |
| `area:provider` | provider契約・stream・model設定・cache |
| `area:tools` | 編集・Git・検索・tool出力 |
| `area:tui` | 端末UI・入力・表示・操作 |
| `area:delivery` | 配布・実行環境・headless interface |
| `area:quality` | 回帰試験・評価・採否判断 |
| `area:docs` | 設計文書・計画・TODOの整合 |

起票時: P1 10件・P2 25件・P3 1件。状態はready 12件・blocked 23件・deferred 1件。P2の保存基盤 [SOC-012 / #64](https://github.com/chansuke/socium/issues/64) はP1の [SOC-005 / #57](https://github.com/chansuke/socium/issues/57) の前提なので、数値だけで後回しにしない。

## 依存関係の意味

- `depends_on` / blocked by: このIssueの受け入れ条件を完了するのに必要な前提。複数ある場合は全て必要（AND）。理由は個別Issue本文とJSONの各辺に記録した。
- `blocks`: このIssueの完了によって進められる後続。`depends_on` の逆方向で、追加の条件ではない。
- 前提 → 後続をGitHubのネイティブ依存関係として登録。本文にも双方向のリンクがある。
- 40件の親子関係は進捗集約。親Issue #52のcloseを子の前提にしない。
- 設計・fixture作成は前提完了前でも進められる。契約未成立のまま後続を完了扱いにしない。
- 既存 #15（プロジェクト指示）、#16（要約）、#17（memory）は関連提案として参照する。これらへの参照だけでは必須依存にしない。
- 基盤の全面完成を待たずに各修正の回帰試験を作る。SOC-030を全Issueの必須前提にしていないのはこのため。

登録方法はGitHub公式の[Issue dependencies API](https://docs.github.com/en/rest/issues/issue-dependencies)と[Sub-issues API](https://docs.github.com/en/rest/issues/sub-issues)に従う。

## 人間・エージェント共通の進め方

1. 親Issueの「現在の着手可能Issue」から候補を選ぶ。SOC IDで原票を開き、現HEADで指摘を再確認する。
2. Issue本文・前提Issue・ネイティブ依存を読む。担当者と着手を宣言し、状態ラベルを `status:in-progress` に置き換える。依存が残る設計先行の場合はその範囲をコメントで明記する。
3. 受け入れ条件と対象外を守って実装・検証する。未検証のOS・実API・費用を隠さず、PR/commit・実行コマンド・結果を記録する。
4. 全条件を検証してcloseし、状態ラベルを外す。後続の未完了前提がなくなったら `status:blocked` を `status:ready` に置き換える。自動更新のworkflowは導入していない。
5. 却下・重複で前提をcloseした場合、単純に解除しない。代替契約や代替Issueを決め、依存辺と本文を更新する。
6. SOC-036は評価後の採否判断まで `status:deferred` を維持する。全拡張の一括実装を完了条件に追加しない。

状態ラベルを複数併用しない。実装中に新しい必須前提が発生したら、作業を止めるか設計だけ進めるかを記録して状態を更新する。受け入れ条件・前提を変更した場合は、Issue本文とGitHubの依存関係、ローカル原票、JSONを同時に整合させる。

## 機械可読情報

`github-issues.json` は `schema_version: 1`。各要素は `id`、`number`、`url`、`source`、`priority`、`phase`、`labels_at_creation`、`status_at_creation`、`depends_on`、`blocks` を持つ。

- `depends_on` の各要素: 前提のSOC ID、Issue番号、依存理由。
- `blocks`: 逆向き参照。辺の定義は `depends_on` と一致させる。
- Issue本文のHTMLコメント `socium-ticket` にもSOC ID・基準commit・依存IDを記録した。
- JSONをタスクキューの現在状態として使わない。GitHubの最新state/labelsと依存Issueの状態を取得して判断する。
- `phase` は推奨段階（0: 文書整合、1: 権限/実行、2: 記録/保存、3: 編集/Git、4: 設定/context/tool、5: provider/session/配布、6: 評価/UX、7: 拡張採否）。トポロジカルな着手順の代わりにはならない。

参照専用の取得例:

```bash
gh issue list --repo chansuke/socium --state open --label 'review:2026-11-11' --label 'status:ready' --json number,title,labels,url
gh api repos/chansuke/socium/issues/57/dependencies/blocked_by
gh api 'repos/chansuke/socium/issues/52/sub_issues?per_page=100'
```

## 資料と公開範囲

原レポートと再現証拠は時点固定資料としてGit管理している。問題・対応範囲・受け入れ条件・検証方法はIssue本文にも転記し、レビュー時の実装リンクは基準commitに固定した。現在の挙動は運用ドキュメント、完了状態はGitHubを参照する。

元の観察用15テストは欠陥があると成功する。修正の回帰試験では期待を安全な挙動へ反転する。詳細な根拠の原本は `docs/review-evidence/2026-11-11/` に保持する。

