---
number: 5
title: 実行の事実から導いたシグナルでエージェントの状態をActivity欄に示す
status: accepted
date: 2026-11-12
links:
  - target: 2
    kind: amends
---

# 実行の事実から導いたシグナルでエージェントの状態をActivity欄に示す

## Context and Problem Statement

ADR 2 で、Activity 欄に思考・tool 呼び出し・結果・token 使用量を流すようにした。それでも、長い turn では「今は調べているのか、直しているのか」「同じことを繰り返していないか」「失敗の後に方針を変えたか」を、利用者が item を読み比べて判断する必要がある。API は生の chain of thought を返さないので（ADR 3）、モデルの内心をそのまま見せることはできない。

## Decision Drivers

* エージェントの状態を一目で分かるようにする。
* 推測で人格を与えず、表示の根拠を Activity の item として辿れるようにする。
* TUI と headless 実行で同じ判定を使い、再生した turn でも同じ表示になる。

## Considered Options

* core が `ActivityEvent` の列から決まった規則でシグナルを導き、TUI は表示だけを行う。
* 安価なモデルに Activity を要約させ、状態を文章で示す。
* シグナルを出さず、生のイベントだけを見せる。

## Decision Outcome

Chosen option: 「core が決まった規則でシグナルを導く」。追加の API 呼び出しも費用も要らず、判定の根拠が event そのものなので検証できるため。

* `socium_core::activity::SignalTracker` が `ActivityEvent` を一つずつ受け取り、状態の変化を返す。保存はせず、再生時も同じ event から導き直す。
* 段階（phase）: 直近に始まった tool の種類で決める。読む・探す・git の参照は `investigating`、`apply_patch` と text editor は `editing`、`run_command` は `verifying`、`git_commit` は `committing`。model の応答待ちは `thinking`、turn の終了で `idle`。Activity 欄の枠のタイトルに出す。
* 堂々巡り: 一つの turn で、同じ tool を同じ入力の要約で 3 回始めたら、その時点で一度だけ知らせる。
* 回復: tool が失敗した直後に、別の tool か別の入力で tool を始めたら知らせる。
* 沈黙: progress update も応答本文も無いまま `tool_use` で終わった round が 5 回続いたら、turn ごとに一度だけ知らせる。
* 消費: turn の見出しに、その turn の全 round の出力 token の合計を出す。
* シグナルは item として Activity 欄に挟み、どの item から導いたかが前後で分かるようにする。

### Consequences

* Good, because 状態の要点が一行で分かり、根拠も同じ画面にある。
* Good, because 判定が決定的で、単体テストで固定できる。
* Bad, because 規則で拾えない状態（方針の迷いなど）は示せない。閾値（3 回、5 round）は経験則で、調整が要るかもしれない。
* Neutral, because 段階は直近の tool だけで決まるので、並列に複数の tool を呼ぶ round では最後の tool の種類になる。

### Confirmation

core の単体テストで、固定した event 列から段階の遷移、堂々巡り・回復・沈黙の発火と一度だけであること、turn をまたいでリセットされることを確認する。TUI はシグナル item と枠のタイトルの描画を TestBackend で確認する。

## Pros and Cons of the Options

### core が規則で導く

* Good, because 費用が無く、決定的で、headless でも使える。
* Bad, because 表現できる状態が規則の数に限られる。

### モデルに要約させる

* Good, because 状態を柔軟な文章で表せる。
* Bad, because round ごとに API 呼び出しと費用が増える。要約の誤りを根拠で確かめにくい。

### シグナルを出さない

* Good, because 実装が要らない。
* Bad, because 状態の判断を利用者に任せたままになる。

## More Information

設計文書 §6 の「導出シグナル」を具体化する。閾値を変えるときは、この ADR を改める。
