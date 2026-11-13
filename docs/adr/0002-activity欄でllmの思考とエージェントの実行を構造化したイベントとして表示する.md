---
number: 2
title: Activity欄でLLMの思考とエージェントの実行を構造化したイベントとして表示する
status: accepted
date: 2026-11-12
links:
  - target: 5
    kind: amended-by
---

# Activity欄でLLMの思考とエージェントの実行を構造化したイベントとして表示する

## Context and Problem Statement

Socium の実行中に、モデルが何を考え、何をしようとし、結果どうなったかを読み取れるようにしたい。Claude Code や Codex より詳しく見せ、Socium の強みにする。

現状の Activity 欄（`crates/tui/src/view/mod.rs` の `draw_activity`）は、`AgentProgress` を TUI が文字列にした `Vec<String>` の末尾数行を出すだけで、スクロールも展開もできない。思考は transcript の思考カードに流れ、tool の開始と終了は transcript と Activity の両方に出る。provider の stream parser は tool 入力の差分（`input_json_delta`）、block の境界、round ごとの usage と stop reason を UI に渡していない。

## Decision Drivers

* 思考の要約、tool 呼び出しの間の progress update、組み上がっていく tool 入力をリアルタイムに見せる。
* round ごとの stop reason・token 使用量・TTFT・所要時間と、tool の成否・所要時間を見せる。
* 整形表示から生の SSE event まで降りられる。
* TUI と headless 実行（SOC-031）が同じ実行事実を返す。
* 未完成の tool 入力で tool を実行しない既存の保証を崩さない。

## Considered Options

* core が provider 非依存の `ActivityEvent` を発行し、TUI は turn → round → item の木として表示する。
* 既存の `AgentProgress` を保ったまま、TUI 側の文字列整形を増やす。
* 生の SSE event をそのまま Activity 欄に流す。

## Decision Outcome

Chosen option: 「core が `ActivityEvent` を発行し、TUI は木として表示する」。意味づけを core に置くと、TUI・headless・永続化（ADR 4）が同じ事実を共有でき、表示の改良が core の契約を変えないため。

* provider 層の `DeltaEvent` を、block の開始・差分・終了と message の終了（stop reason、usage）を運ぶ `ProviderEvent` に広げる。生の SSE は round ごとの上限付き ring buffer に残す。
* agent 層は `turn_id`・`round`・block index・`tool_call_id` と turn 開始からの経過時間を付けた `ActivityEvent` を発行する。種類と項目は設計文書 §4.2 のとおり。
* tool 定義に `eager_input_streaming: true` を付け、部分 JSON を表示専用の差分として流す。実行には従来どおり `finish()` で完成し検証した入力だけを使う。
* thinking block は、直後の block が `tool_use` なら progress update、それ以外は reasoning と block の終了時に判定し、確定値を event に載せる。中断の番兵（`This part of the response was interrupted before it finished.`）は `interrupted` として扱う。
* 思考は Activity 欄だけに出す。transcript の思考カードは 1 行のインジケーターにし、transcript の tool 開始・終了の `System` 行は廃止して Activity 欄に一本化する。
* `Ctrl+A` で Activity 欄にフォーカスし、`↑` `↓` `PgUp` `PgDn` で選択とスクロール、`Enter` で展開、`r` で選択 round の生 SSE、`z` で全画面にする。末尾を選択しているときだけ自動で追従する。
* 端末の幅が 140 列以上なら Activity 欄を右カラムに置き、それ未満は下段で高さを画面に応じて変える。last command 欄は tool item の展開に吸収する。
* `AgentProgress` は TUI の移行後に重複部分を削る。承認の制御（`ApprovalRequested`）は残す。
* モデル出力・tool 出力・生 SSE は表示前に制御文字を除く。

### Consequences

* Good, because 思考・意図・結果・代償を一つの時系列で読める。
* Good, because 同じ event を headless JSONL と永続化に使える。
* Good, because transcript が人間との対話だけになり読みやすくなる。
* Bad, because core の公開型が増え、`AgentProgress` からの移行期間は二系統の event が並ぶ。
* Bad, because `display: "summarized"` では reasoning と progress update を API 上は区別できず、判定は近似になる。誤判定しても表示の色が変わるだけである。
* Neutral, because `eager_input_streaming` を付けると API は tool 入力を検証しなくなる。Socium は既に完成した JSON を parse してから実行している。

### Confirmation

core は SSE の fixture から `ActivityEvent` の列を検証する（summarized、updates、中断の番兵、複数 tool、usage 未報告）。TUI は Ratatui の `TestBackend` で 80×24 と 140×40、streaming 中、tool 失敗、usage 未報告の描画を確認する。部分 JSON が不正なまま終わった応答で tool が実行されないことをテストする。

## Pros and Cons of the Options

### core が `ActivityEvent` を発行する

* Good, because 表示・永続化・headless で事実が一致する。
* Bad, because 実装量が最も多い。

### TUI 側の文字列整形を増やす

* Good, because 変更が TUI に閉じる。
* Bad, because tool 入力の差分や usage は `AgentProgress` に無く、結局 core を変える必要がある。headless と事実がずれる。

### 生の SSE をそのまま流す

* Good, because 実装が小さく、情報の欠落が無い。
* Bad, because 1 秒に数十行の JSON は読めない。tool 実行・承認・compaction は SSE に含まれない。

## More Information

詳しい仕様は [設計: Activity ストリーム](../design-agent-activity-stream.md)。thinking の表示設定は ADR 3、永続化は ADR 4。関連チケットは SOC-020、SOC-026、SOC-027、SOC-031。
