---
number: 17
title: TUIなしで一つのturnを実行しJSON Linesで報告する
status: proposed
date: 2026-11-15
links:
- target: 19
  kind: amendedby
---

# TUIなしで一つのturnを実行しJSON Linesで報告する

## Context and Problem Statement

socium を動かす経路は TUI だけで、プロンプトや tool の変更がよくなったかを機械的に測れなかった (2026-11-14 のレビュー N14、SOC-031)。ADR 2 と ADR 4 は、Activity の event を headless の JSONL に使えるようにしていた。どの範囲の headless の入出力を持つかを決める。

## Decision Drivers

* TUI なしで 1 つのタスクを実行し、tool、usage、結果を機械的に取れる。
* stdout にはスキーマに従った行だけを出し、診断は stderr に分ける。
* 承認に答える人がいないことを、黙って許可しない。取り消しと出力先の切断で安全に止まる。
* TUI と同じ core の経路を通る。

## Considered Options

* `-p` で 1 turn を実行し、版付きの JSON Lines を stdout に出す
* stdin と stdout で制御と応答をやり取りする RPC モードを持つ
* ライブラリの API (SDK) として公開する

## Decision Outcome

Chosen option: 「`-p` で 1 turn を実行し、版付きの JSON Lines を出す」。評価と自動化に要るのは 1 つの依頼の結果と経過で、それは ADR 2 の event と同じ事実を流せば足りる。

* `socium -p PROMPT` は、TUI を起動せずに PROMPT を 1 turn として実行する。`-p -` は stdin からプロンプトを読む。`-c` と `-r <ID>` と組み合わせて保存した会話を続けられる。`-r` の選択画面とは組み合わせられない。
* stdout の各行は JSON で、`v` にスキーマの版 (今は 1)、`type` に種類を持つ。
  * `started`: セッション ID とモデルの alias と ID。
  * `activity`: `event` に Activity の event。セッションの activity log と同じ形。
  * `finished`: `ok`、返答の `text` か `error`、`notices`、会話の `usage` と `cost_usd`、セッション ID。
* 起動時の警告などの診断は stderr に出す。
* 承認が要る呼び出しは、答える人がいないので拒否し、`config.toml` で許可する方法をモデルに伝える (ADR 13)。
* Ctrl+C (SIGINT) か stdout への書き込みの失敗で turn を取り消し、`finished` を出して終わる。
* 終了コードは、成功 0、失敗 1、使い方の誤り 2、取り消し 130。

### Consequences

* Good, because 固定のタスクを並べて評価や回帰の確認ができる。
* Good, because TUI と同じ event なので、表示と記録と headless の事実が一致する。
* Bad, because 実行中に指示を足したり承認に答えたりする経路は無い。要るなら RPC モードを別に決める。

### Confirmation

TUI の crate の統合テストで、実際のバイナリを環境を空にして起動し、Anthropic の stream 形式で答えるローカルのサーバーに対して、stdout のすべての行が版付きの JSON で `started`・`activity`・`finished` の順に出ること、承認が要るコマンドが実行されずモデルに `config.toml` の案内が返ること、SIGINT で 130 で終わり `finished` が `ok: false` になることを確かめる。

## Pros and Cons of the Options

### `-p` と JSON Lines

* Good, because 小さく、シェルと他の言語から扱える。
* Neutral, because 1 回の実行が 1 turn に限られる。

### RPC モード

* Good, because 承認や追加の指示をやり取りできる。
* Bad, because 制御のスキーマ、部分行、切断、背圧の扱いが要り、今の需要に比べて大きい。

### SDK

* Good, because Rust から直接使える。
* Bad, because core の API を公開の契約にすることになる。

## More Information

[SOC-031](../tickets/SOC-031-headless-jsonl.md)。2026-11-14 のレビューの N14。event の形は [ADR 2](0002-activity欄でllmの思考とエージェントの実行を構造化したイベントとして表示する.md) と [ADR 4](0004-activityの記録をセッション単位のjsonlに保存し起動時に再生する.md)。
