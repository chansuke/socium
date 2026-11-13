---
number: 4
title: Activityの記録をセッション単位のJSONLに保存し起動時に再生する
status: accepted
date: 2026-11-12
---

# Activityの記録をセッション単位のJSONLに保存し起動時に再生する

## Context and Problem Statement

ADR 2 の Activity 欄は TUI のメモリにしかなく、Socium を再起動すると前の turn で何を考え何をしたかが消える。会話履歴（`.socium` 配下の会話 snapshot）は reasoning の本文と tool の入出力を持つが、時刻・所要時間・usage・承認の待ち時間は持たない。named session（SOC-026）は未実装で、今はワークスペースごとに一つの会話がある。

## Decision Drivers

* 再起動後も直近の Activity を同じ見た目で見られる。
* 書き込みが turn の実行を遅らせたり、失敗させたりしない。
* secret が混じりうる面を広げない。
* 将来の named session と headless JSONL（SOC-031）に載せ替えられる。

## Considered Options

* `ActivityEvent` を append-only の JSONL に書き、起動時に読んで再生する。
* 会話 snapshot に Activity の情報を足す。
* 永続化せず表示だけにする。

## Decision Outcome

Chosen option: 「append-only の JSONL に書き、起動時に再生する」。ADR 2 の event をそのまま使え、会話 snapshot の原子的な保存（SOC-012）に手を入れずに済むため。

* 保存先は `workspace_state_path` で得る `.socium/activity.jsonl`。今の「セッション」はワークスペースの会話一つに対応し、SOC-026 で session ごとのファイルに分ける。
* 1 行 1 `ActivityEvent` とし、先頭行に schema version を書く。`ThoughtDelta`・`ToolIntentDelta` は保存時に block 単位でまとめ、差分を一つずつは書かない。
* tool の結果は本文を書かず、要約・byte 数・成否・所要時間だけを書く。生 SSE は保存しない。
* 書き込みは turn の区切りと block の終了でまとめて行う。書き込みの失敗は Activity 欄に警告を出すだけで、turn は失敗させない。
* ファイルが 16 MiB を超えたら `activity.1.jsonl` に回して新しく始め、古いものは一世代だけ残す。`/clear` でも同じく回す。
* 起動時は末尾から直近 20 turn 分を読み、ActivityModel を組み立てる。最後の行が途中で切れていたら（クラッシュ時）その行だけ捨てる。不明な version や壊れた行があっても起動は止めない。
* 再生した turn は表示上で「過去の turn」と分かるようにする。

### Consequences

* Good, because 再起動後も思考と実行の流れを振り返れる。
* Good, because headless JSONL の schema と同じ event を共有できる。
* Bad, because 会話 snapshot と Activity のファイルが別になり、片方だけ残る状態がありうる。再生は Activity の内容だけを信じ、会話との突き合わせはしない。
* Bad, because reasoning の要約は会話 snapshot と重複して保存される。

### Confirmation

core の単体テストで、保存→再読込で同じ木になること、途中で切れた最終行と不明な version を読み飛ばすこと、16 MiB での回転、tool 結果の本文が書かれないこと、書き込み失敗で turn が失敗しないことを確認する。

## Pros and Cons of the Options

### append-only の JSONL

* Good, because 追記だけで済み、途中で落ちても前の行は残る。
* Bad, because 会話 snapshot と別に容量の管理が要る。

### 会話 snapshot に足す

* Good, because 一つのファイルで整合する。
* Bad, because snapshot は turn ごとに全体を原子的に書き直すので、delta の多い Activity を足すと保存が重くなる。SOC-012 の保存境界に影響する。

### 永続化しない

* Good, because 実装が要らない。
* Bad, because 再起動で記録が消え、振り返りができない。

## More Information

ADR 2 の event を保存の単位にする。SOC-026（named session）の実装時に保存先を session ごとに分け、この ADR を改める。
