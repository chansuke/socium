---
number: 18
title: エージェントの計画を会話の履歴から導きActivity欄の第二の表示に示す
status: proposed
date: 2026-11-17
links:
  - target: 2
    kind: amends
  - target: 5
    kind: amends
  - target: 17
    kind: amends
---

# エージェントの計画を会話の履歴から導きActivity欄の第二の表示に示す

## Context and Problem Statement

長い作業で、利用者は「全体のどこまで進んだか」を Activity の item から読み取るしかなかった。モデルにも、圧縮の後まで残る作業の段取りがなかった。DeepSeek Harness (dsh) の Timeline は、モデルが自分で更新するタスクの木と重み付きの進捗でこれを解いている (deepseek-ai/deepseek-harness 39163dd、packages/experimental/timeline)。SOC-036 の方針に沿って、dsh から何を取り込み、計画をどこに持ち、TUI のどこに出すかを決める。

## Decision Drivers

* rewind、再開、/clear、圧縮、クラッシュ修復の後も、計画が会話と食い違わない。
* 表示のための有料の API 呼び出しを増やさない (ADR 5)。
* prompt cache の前提である「履歴は伸びるだけ」を崩さない。
* 80x24 の端末でも使え、Activity 欄の既存の操作を変えない。

## Considered Options

* 計画を `update_plan` の呼び出しから導く。保存はしない。
* 計画をセッションごとのファイルか会話の状態に保存する。
* 計画を毎回 system prompt に入れる (dsh の方式)。
* TUI では、Timeline を Activity 欄の第二の表示にして Ctrl+T で切り替える。または Activity 欄の横に常に置く。

## Decision Outcome

計画は保存しない。会話の中で成功した `update_plan` の呼び出しを、決まった規則で畳み込んで導く。TUI では Activity 欄の第二の表示にし、幅のある端末では Activity の上に積む。

* ツールは `update_plan` の一つだけにする。一回の呼び出しで複数のタスクを変え、全部を受け入れるか、何も変えずに理由を返すかのどちらかにする。タスクは木で、状態は pending、active、blocked、done、dropped。葉は `[済み, 全体]` の単位を持つ。重みは兄弟の間の比で、子に分けても親の取り分は変わらない。状態、単位、親、重みを変えるときと、blocked や dropped にするときは note を必須にする。
* 計画は `socium_core::plan::replay` が会話から導く。結果が保存されていない呼び出しと、エラーで終わった呼び出しは数えない。そのため rewind、再開、/clear、クラッシュ修復のための計画のコードはない。
* 計画を書けるのは main agent だけにする。サブエージェントには `update_plan` を渡さない (`MAIN_AGENT_TOOLS`)。
* モデルが計画を見る経路は追記だけにする。各呼び出しの結果には、見出し、変わった点、未完了のタスクの一覧を返す。圧縮の要約には、切れ目の時点の計画を末尾に付ける (完了した計画は付けない)。system prompt と送信済みのメッセージは変えない。
* 未完了の計画があるのに 8 ラウンド更新がないときは、その round の最後の結果に催促を一度だけ追記し、`plan_nudged` の event で知らせる。dsh の 24 回は `MAX_TOOL_ROUNDS` の 20 を超えるので、このままでは働かない。
* 表示には `plan_updated` の event を使う。結果が保存された後に送るので、TUI、activity.jsonl、headless が同じ事実を見る (ADR 17)。turn の終わりと履歴が変わる操作の後は、TUI が `AgentService::plan()` で導き直す。
* TUI は Ctrl+T で Activity と Timeline を入れ替える。focus と全画面は欄の側が持ち、Ctrl+A、z、Esc は両方の表示で同じ意味になる。計画があるとき、Activity の枠のタイトルに進捗 (`plan 32% 2/7`) を出す。140 桁以上で欄の高さが足りるときは、Timeline を Activity の上に自然な高さ (欄の 45% まで) で積む。このとき Ctrl+T はキーと focus の色の行き先だけを切り替える。
* `update_plan` は段階 (phase) を変えない。

dsh から取り込まないもの: 定時の LLM レポート (有料の呼び出し)、タスクごとの議論と返信、優先度の通知、複数の木の切り替え、export/import と fork、読み取り用の別ツール (結果が未完了の一覧を返すので要らない)、残り時間の見積もり、paused。

### Consequences

* Good, because 計画が会話と同じ一つの記録から決まり、どの操作の後でも食い違わない。
* Good, because 追加の API 呼び出しがなく、prompt cache を壊さない。
* Good, because 80x24 では片方の表示に欄の幅を全部使える。
* Bad, because 履歴の読み直しのたびに会話を一度なめる。タスクは 40 個までなので費用は小さい。
* Bad, because 将来 `Plan::apply` の規則を厳しくすると、古いセッションの呼び出しが適用できなくなる。その呼び出しは飛ばし、turn の始めに Warning で知らせる。
* Neutral, because 計画は一つのセッションに一つ。新しい goal は前の計画を置き換える。

### Confirmation

core の単体テストで、検証、原子性、冪等性、重み付きの進捗、履歴のすべての接頭辺から導いた計画と turn 中の計画の一致 (property test) を確かめる。`AgentService` を通したテストで、`plan_updated` の順序、rewind、再開、圧縮の要約、サブエージェントの除外、催促を確かめる。TUI は TestBackend で 80x24 の入れ替えと 160x40 の積み重ねを、headless は実バイナリで `plan_updated` と `finished.plan` を確かめる。

## Pros and Cons of the Options

### 履歴から導く

* Good, because 保存するものがなく、ライフサイクルの各操作に計画のコードが要らない。
* Bad, because 規則を変えると古い履歴の読み方が変わる。

### 保存する

* Good, because 規則が変わっても過去の計画をそのまま読める。
* Bad, because rewind、/clear、再開、クラッシュの各経路で、会話と計画の保存を揃え続ける必要がある。

### system prompt に入れる

* Good, because モデルが常に最新の計画を見る。
* Bad, because 計画が変わるたびに会話全体の cache を作り直す。

### Activity 欄の横に常に置く

* Good, because 計画と実行を同時に見られる。
* Bad, because 80x24 では下の帯が 5 行しかなく、二つに割るとどちらも読めない。幅のある端末で積む形として一部を取り入れた。

## More Information

設計の比較と統合の記録は、2026-11-17 の architect の作業 (候補 A と B、判定) にある。閾値 (8 ラウンド、40 タスク、45%) を変えるときは、この ADR を改める。
