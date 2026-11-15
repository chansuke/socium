---
number: 14
title: 応答ごとに出所を記録しthinkingは読める送り先にだけ送る
status: proposed
date: 2026-11-15
---

# 応答ごとに出所を記録しthinkingは読める送り先にだけ送る

## Context and Problem Statement

thinking の署名は、それを出したモデルと、その時点までの会話の接頭辞にしか通らない。ADR 6 と ADR 10 は、モデルを切り替えたときと、違うモデルでセッションやサブエージェントを再開したときに、生の履歴から thinking を取り除くとしていた。これには二つの問題があった (2026-11-14 のレビュー N10)。取り除いた thinking は元のモデルに戻しても返らない。そして履歴のどの応答をどのモデルが出したかを記録しないので、DeepSeek から Opus に切り替えたとき、DeepSeek の thinking を Opus に送るかどうかを決められない。Anthropic の API は、今のモデルが読めない thinking を課金せずに落とすので、自分で外す必要も無い。

## Decision Drivers

* 生の履歴を書き換えない (追記だけにする)。
* 送り先が読めない thinking を送らない。読める thinking は返す。
* 古い形式で保存した会話も読める。

## Considered Options

* 応答ごとに出所 (provider とモデル) を記録し、送るときに送り先が読めるものだけを残す
* 切り替えのたびに生の履歴から thinking を取り除く (従来)
* thinking をすべて送り、送り先の判断に任せる

## Decision Outcome

Chosen option: 「応答ごとに出所を記録し、送るときに選ぶ」。生の履歴を保ったまま、送り先ごとに正しい集合を送れる。

* assistant の message に `origin: {provider, model}` を記録する。provider は registry の provider 名、model はモデル ID。JSON には `origin` として保存し、API への送信には含めない。出所の無い古い message と、user の message は `None`。
* 送るときに、provider に `reads_thinking_from(origin)` を尋ね、読めない thinking を含む message からだけ thinking を外した写しを送る。何を外すかは送り先の provider とモデルだけで決まるので、あるモデルが出した thinking は、いつも今と同じ写しを接頭辞にして作られている。外しても、そのモデル自身の thinking は有効なまま。
  * Anthropic の API (`https://api.anthropic.com`) は、同じ provider の Claude モデルが出したものなら読む。API が読めないものを落とす。
  * Anthropic 互換の他の endpoint は、同じ provider の同じモデルが出したものだけ読む。
  * OpenAI Responses は、同じモデルの暗号化された reasoning だけを返す。
  * OpenAI Chat Completions は thinking を受け付けないので、何も送らない。
  * 出所の無い thinking は、どこにも送らない。
* `/model` の切り替え、セッションの再開、サブエージェントの再開で、生の履歴から thinking を取り除かない。
* 要約の後に残す message の thinking を送らない規則 (`thinking_valid_from`) は、別の理由 (接頭辞が変わる) なのでそのまま。

### Consequences

* Good, because Opus から DeepSeek に切り替えて戻ると、Opus の thinking がまた送られる。
* Good, because 履歴が追記だけになり、プロンプトキャッシュの接頭辞も変わらない。
* Bad, because 保存する会話に出所の分だけ項目が増える。
* Neutral, because 出所の無い古い thinking は、どこにも送らなくなる。

### Confirmation

`types.rs` と信頼性テストで、送信の JSON に `origin` が入らないこと、読めない thinking だけが写しから外れ生の履歴は残ること、出所の無い thinking を送らないことを確かめる。モデル切り替えのテストで、Sonnet の thinking を DeepSeek に送らず、履歴と保存には残し、Opus に切り替えると送ることを確かめる。

## Pros and Cons of the Options

### 出所を記録して送るときに選ぶ

* Good, because 生の履歴が正のまま。
* Good, because provider ごとの規則を provider の実装に置ける。

### 生の履歴から取り除く (従来)

* Good, because 実装が単純。
* Bad, because 元のモデルに戻しても返らない。誰の thinking かが分からない。

### すべて送る

* Good, because 判断が要らない。
* Bad, because Anthropic 以外の endpoint が他のモデルの署名を拒否しうる。Chat Completions には送る場所が無い。

## More Information

2026-11-14 のレビューの N10。[ADR 6](0006-モデルを接続形式と接続先の組で登録しmodelコマンドで切り替える.md) と [ADR 10](0010-サブエージェントのモデル選択と並行実行と記録と再開.md) の thinking を取り除く部分を置き換える。
