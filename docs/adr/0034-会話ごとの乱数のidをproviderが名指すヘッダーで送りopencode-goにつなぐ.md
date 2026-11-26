---
number: 34
title: 会話ごとの乱数のIDをproviderが名指すヘッダーで送りOpenCode Goにつなぐ
status: proposed
date: 2026-11-26
links:
- target: 6
  kind: amends
- target: 23
  kind: amends
---

# 会話ごとの乱数のIDをproviderが名指すヘッダーで送りOpenCode Goにつなぐ

## Context and Problem Statement

OpenCode Go は定額で、GLM、Kimi、Qwen、DeepSeek、Claude Haiku、GPT、Gemini など数十のモデルを使える。ADR 23 の `socium models import opencode-go` で、Go のモデルを wire ごとの provider として取り込める。

しかし Go は、OpenCode 以外のクライアントにも次の二つを求める ([OpenCode Go の文書](https://opencode.ai/docs/go))。

* 会話ごとに変わらない session ID を `x-opencode-session` ヘッダーで送る。
* 自分の名前を `User-Agent` で名乗る。

socium はどちらも送っていなかった。2026-11-26 に、偽のキーで Go の Gemini の口へ送ったところ、ヘッダーのない request は認証より前に `MissingSessionID` で 400 になった。ヘッダーを付けると、どの wire の口でも 401 (Invalid credential) が返り、認証の段まで進んだ。

socium の session ID (`.socium/sessions/<id>/` の名前) は会話を始めた時刻から作る。そのまま送ると、会話をいつ始めたかを送り先に知らせることになる。

## Decision Drivers

* Go の契約者が、取り込んだ Go のモデルをすべて使える。
* 送り先が求めないものは送らない。
* 会話の再開と `/new` で、送り先から見た会話の区切りが socium の会話と揃う。
* 手で書き換えた状態ファイルから、任意の文字列を request のヘッダーに入れられない。

## Considered Options

* provider に `session_header` を置き、会話ごとの乱数の ID を送る
* すべての provider に `x-opencode-session` を送る
* provider に任意のヘッダーを書ける `headers` を置く
* socium の session ID をそのまま送る

## Decision Outcome

Chosen option: 「provider に `session_header` を置き、会話ごとの乱数の ID を送る」。

* `models.toml` の provider は `session_header = "x-…"` を持てる。名前は `x-` で始まるものに限る。`x-api-key` と `x-goog-api-key` は、キーを送るヘッダーなので受け付けない。
* ID は 128 bit の乱数を base64url で表した 22 文字で、会話の状態 (`conversation.json` の `request_session`) に保存する。会話を再開すると同じ ID を使い、`/new` で新しくする。この項目がない状態ファイルや、形の違う値が入った状態ファイルを読んだときも、新しく作る。
* 主エージェントの turn、要約、サブエージェントの request はすべて、その会話の ID を送る。
* `socium models import` は `opencode-go` の provider すべてに `session_header = "x-opencode-session"` を書き出す。Zen (`opencode`) はヘッダーなしで認証の段まで進んだので、書き出さない。
* すべての request に `User-Agent: socium/<version>` を付ける。

### Consequences

* Good, because Go の契約者は、取り込んだ Go のモデルを `/model` で選んで使える。
* Good, because ヘッダーを送るのは名指した provider だけで、ID からは会話を始めた時刻も socium の session ID も分からない。
* Bad, because 会話の状態ファイルに項目が一つ増える。
* Bad, because Go がヘッダーの名前や条件を変えると、取り込み直すか `models.toml` で直す必要がある。

### Confirmation

単体テストで次を確かめる。

* 4 つの wire がどれも、`session_header` を名指した provider にだけ、会話の request に ID のヘッダーを付ける。`User-Agent` も付ける。ID は body に入れない。
* registry が `session_header` を読み、`x-` で始まらない名前とキーのヘッダーを拒む。
* 一つの turn の各 request が、その会話の ID を持つ。
* ID が再開で変わらず、`/new` で変わり、ない値や形の違う値は新しく作られる。
* `opencode-go` の取り込みでは、どの provider にもヘッダーが書かれ、`opencode` では書かれない。

Go の実際の応答は、契約者のキーを使って `SOCIUM_LIVE_MODEL` の live テストで確かめる。

## Pros and Cons of the Options

### `session_header` と会話ごとの乱数の ID

* Good, because 求める送り先にだけ、時刻の分からない ID を送る。
* Bad, because 状態ファイルと request の経路に項目が増える。

### すべての provider に `x-opencode-session` を送る

* Good, because 設定が要らない。
* Bad, because 求めていない送り先にも、会話を結び付けられる ID を渡す。

### 任意のヘッダーを書ける `headers`

* Good, because ほかの送り先の要求にも使える。
* Bad, because 会話ごとに変わる値は書けず、ID の生成と保存は別に要る。キーを送るヘッダーを上書きできないようにする検査も増える。

### socium の session ID をそのまま送る

* Good, because 状態ファイルに項目を足さずに済む。
* Bad, because 会話を始めた時刻を送り先に知らせる。

## More Information

provider と wire の組は [ADR 6](0006-モデルを接続形式と接続先の組で登録しmodelコマンドで切り替える.md)、取り込みは [ADR 23](0023-models-devのカタログを利用者が実行するコマンドでmodels-dに書き出す.md) による。Go のどのモデルを使えるかは、OpenCode の Console の Privacy 設定にもよる (DeepSeek と LongCat には Global、Muse Spark Contributor には学習に使う送り先の許可が要る)。
