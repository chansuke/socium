---
number: 3
title: adaptive thinkingの表示を既定でsummarizedにしTHINKING_BUDGET_TOKENSを廃止する
status: accepted
date: 2026-11-12
---

# adaptive thinkingの表示を既定でsummarizedにしTHINKING_BUDGET_TOKENSを廃止する

## Context and Problem Statement

既定モデルの Claude Opus 5.5 は thinking を無効にできず、thinking の表示（`thinking.display`）の既定は `"omitted"` である。Socium は `thinking` を送っていないので、thinking block の本文は常に空で、tool 呼び出しの間の progress update も空になる。このため ADR 2 の Activity 欄に出す思考が届かず、長い tool loop が無言に見える。

また `THINKING_BUDGET_TOKENS` を設定すると `{type: "enabled", budget_tokens}` を送る（`crates/core/src/providers/anthropic/mod.rs`）。Opus 5.5 はこれを 400 で拒否するので、この環境変数を設定するとすべてのリクエストが失敗する。

API は生の chain of thought を返さない。返せるのは reasoning の要約と progress update で、どの表示設定でも思考の量と課金は変わらない。

## Decision Drivers

* Activity 欄に出せる思考の情報量を最大にする。
* 既定モデルで失敗する設定を残さない。
* 表示の量は利用者が選べるようにする。

## Considered Options

* 既定を `"summarized"` にし、`THINKING_DISPLAY` で `updates`・`omitted` に変えられるようにする。
* 既定を `"updates"`（beta `thinking-display-updates-2026-08-18`）にする。
* 既定を `"omitted"` のままにする。

## Decision Outcome

Chosen option: 「既定を `"summarized"` にする」。reasoning の要約と progress update の両方が返り、Activity 欄の情報量が最も多いため。

* 常に `thinking: {type: "adaptive", display: <THINKING_DISPLAY>}` を送る。`THINKING_DISPLAY` は `summarized`（既定）・`updates`・`omitted` を受け付け、それ以外は起動時の設定エラーにする。
* `updates` のときだけ `anthropic-beta: thinking-display-updates-2026-08-18` を付ける。
* `EFFORT`（`low`・`medium`・`high`・`xhigh`・`max`）を設定したときだけ `output_config.effort` を送る。未設定なら API の既定（Opus 5.5 では `medium`）に任せる。
* `THINKING_BUDGET_TOKENS` は廃止する。設定されていたら無視し、廃止されたことと代わりの `EFFORT` を警告する。
* thinking block は今どおり signature ごと会話履歴に戻す。

### Consequences

* Good, because 既定のままで思考の要約が Activity 欄に流れる。
* Good, because 既定モデルで 400 になる設定が無くなる。
* Bad, because summarized では reasoning と progress update が混在し、ADR 2 の判定が近似になる。
* Neutral, because 表示の設定は課金を変えない。要約の文字列が増える分だけ会話履歴の保存量は増える。

### Confirmation

provider の単体テストで、各 `THINKING_DISPLAY` に対する request body と beta header、`EFFORT` の有無、不正値の拒否、`THINKING_BUDGET_TOKENS` を送らないことを確認する。実 API での確認（thinking block に本文が入ること）は手動で行い、PR に結果を書く。

## Pros and Cons of the Options

### 既定を `"summarized"` にする

* Good, because 思考の要約と progress update の両方が見える。
* Bad, because 出力が多く、静かな表示を好む利用者には `updates` への切り替えが要る。

### 既定を `"updates"` にする

* Good, because progress update だけを確実に識別できる。
* Bad, because reasoning が隠れ、「何を考えたか」が見えない。beta header に依存する。

### `"omitted"` のままにする

* Good, because 変更が要らない。
* Bad, because Activity 欄に思考がほぼ出ず、ADR 2 の目的を果たせない。

## More Information

Opus 5.5 の thinking と effort の仕様は Claude API の移行ガイド（Migrating to Claude Opus 5.5）による。設定値の検証は SOC-015 の範囲と重なる。
