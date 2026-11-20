---
number: 6
title: モデルを接続形式と接続先の組で登録し/modelコマンドで切り替える
status: accepted
date: 2026-11-13
links:
- target: 22
  kind: amendedby
- target: 23
  kind: amendedby
---

# モデルを接続形式と接続先の組で登録し/modelコマンドで切り替える

## Context and Problem Statement

socium は `ANTHROPIC_API_KEY` と `MODEL` を起動時に環境変数から読み、Anthropic の Messages API だけに接続している。DeepSeek（deepseek-v4.1-flash など）、GLM（glm-5.3 など）、OpenAI の GPT-5 にも切り替えて使いたい。会話の途中で `/model` から選べるようにし、選んだモデルは会話を再開しても保たれるようにしたい。

各社の接続口は形式が揃っていない。DeepSeek と Z.ai（GLM）は Anthropic 互換の endpoint を公開している。OpenAI には Anthropic 互換の口が無く、Chat Completions か Responses API を使う。Ollama Cloud や OpenRouter は多くのモデルを OpenAI 互換の口でまとめて提供するが、OpenAI 本家の GPT-5 は扱わない（Ollama Cloud にあるのは gpt-oss）。

現在の request は Anthropic 専用の項目（adaptive thinking、`output_config.effort`、`eager_input_streaming`、beta header、`text_editor_20250728` 型の tool）を無条件に送る。互換 endpoint はこれらを拒否するか黙って無視する。

## Decision Drivers

* OpenAI 本家の GPT-5 を含め、使いたいモデルに一つの仕組みで届く。
* 接続先を増やすたびに adapter を書かなくて済む。
* 対応していない機能を黙って送らない。最初の有料 request より前に設定の誤りを示す。
* API キーを設定ファイル、履歴、ログ、エラー表示に出さない。
* 選んだモデルと会話履歴の対応を保ち、別のモデルの thinking 署名を送らない。

## Considered Options

* 接続形式（wire）と接続先（endpoint）を分けた registry にモデルを登録する。
* すべてを Ollama Cloud 経由にする。
* すべてを OpenRouter 経由にする。
* ベンダーごとに専用の adapter を書く。

## Decision Outcome

Chosen option: 「wire と endpoint を分けた registry」。wire は Anthropic Messages、OpenAI Chat Completions、OpenAI Responses の三つで足り、接続先は設定で増やせる。Ollama Cloud や OpenRouter も、その中の一つの endpoint として使える。

### 設定

利用者の設定ファイル `$XDG_CONFIG_HOME/socium/models.toml`（未設定なら `~/.config/socium/models.toml`）に provider とモデルを書く。`SOCIUM_MODELS_FILE` でパスを差し替えられる。ファイルが無ければ組み込みの定義だけで、今と同じく Anthropic に接続する。

```toml
default = "opus"

[providers.deepseek]
wire = "anthropic"
base_url = "https://api.deepseek.com/anthropic"
api_key_env = "DEEPSEEK_API_KEY"
auth = "bearer"

[[models]]
alias = "ds-flash"
provider = "deepseek"
id = "deepseek-flash"
context_window = 1000000
max_output_tokens = 32000
```

* provider は `wire`、`base_url`、`api_key_env`、`auth`（`x-api-key` か `bearer`）を持つ。キーそのものは書かず、キーを入れた環境変数の名前だけを書く。
* `base_url` は https に限る。loopback（`localhost`、`127.0.0.1`、`::1`）だけは http を許す。
* モデルは `alias`、`provider`、`id` と、任意で `features`、`context_window`、`max_output_tokens`、`compact_model` を持つ。
* 組み込みの定義として provider `anthropic` と、モデル `opus`（claude-opus-5-5）、`sonnet`（claude-sonnet-5-5）、`haiku`（claude-haiku-5-5）を持つ。設定ファイルで同じ名前を書けば上書きする。

### wire 固有の機能

Anthropic 固有の機能は、モデルの `features` に挙げたときだけ送る。`adaptive_thinking`、`effort`、`eager_input_streaming`、`anthropic_text_editor` の四つ。組み込みの Claude モデルは全部を持ち、利用者が書いたモデルは既定で何も持たない。`anthropic_text_editor` が無いモデルには、text editor tool を同じ名前と同じ入力の、通常の JSON schema の tool として渡す。

### 選択と保存

* `/model` は引数なしで一覧を composer の位置に出し、`/model <alias>` で直ちに切り替える。キーの環境変数が無い provider のモデルは、理由を付けて選べないものとして出す。
* 切り替えは request の実行中には受け付けない。切り替える時点で provider を組み立て、キーと設定を検査する。
* `/model` で選んだ alias だけを `.socium/conversation.json` に保存する。`MODEL` や `default` で決まったモデルは保存しないので、後から `MODEL` を変えればそれが効く。
* 起動時の選択は、保存された alias、`MODEL` 環境変数、設定ファイルの `default`（未指定なら組み込みの `opus`）の順に決める。保存された alias が registry から消えていたり、キーが無く使えなかったりしたら、警告して次に進み、保存を消す。`/model` は利用者が会話の中で選んだ結果なので、常に設定される `MODEL` より優先する。どれも使えなければ、登録順で最初に組み立てられるモデル（キーのあるもの）で起動し、警告する。それも無ければ起動を止める。
* `MODEL` は alias か、registry にあるモデル ID を受け付ける。どちらでもない値は、組み込みの `anthropic` provider のモデル ID とみなす（従来の動作）。

### 履歴の扱い

モデルを変えるときに、履歴の thinking ブロックを取り除いて保存する。thinking の署名はそれを出したモデルにしか通らず、切り替えは turn の間でしか起きないので、取り除いても tool の往復は壊れない。Activity の記録には thinking が残る。

### compaction

要約には、使用中のモデルの `compact_model` を使う。未設定なら使用中のモデル自身を使う。組み込みの `opus` と `sonnet` は `haiku` を指す。`COMPACT_MODEL` 環境変数は廃止し、設定されていたら起動時に警告する。DeepSeek を使っているのに要約だけが Anthropic へ送られ課金される、といった食い違いを防ぐため。

### 段階

1. Anthropic wire の registry、`/model`、`features` による送信の制御、thinking の除去、保存（SOC-041）。DeepSeek と GLM には、それぞれの Anthropic 互換 endpoint で接続する。
2. provider 非依存の契約（SOC-014）と OpenAI Chat Completions の wire（SOC-016）。OpenAI 本家の GPT-5、Ollama Cloud、OpenRouter に接続する。2026-11-13 の実装では、履歴と tool 定義の変換を `openai-chat` の adapter 内に置き、core の型は変えていない。SOC-014 の共通契約は、Responses（SOC-042）で reasoning item を持つ必要が出た時点で定める。OpenAI の現行モデル一覧では GPT-5 系の汎用モデルが外れ、GPT-6 系（`gpt-6-astra` など）が主力になっていたので、設定例はそれに合わせた。
3. OpenAI Responses の wire（SOC-042）。GPT-5 の reasoning を tool の往復をまたいで保つ。2026-11-14 の実装では、`gpt-6-astra` が Chat Completions で tool と reasoning の併用を拒否したため、OpenAI 本家の設定例をこの wire に移した。reasoning item は core の型を増やさず既存の `Thinking` ブロック（`signature` に送り返す item）に載せ、モデル切り替え時の除去も既存の処理に任せた。

### Consequences

* Good, because 接続先の追加が設定だけで済み、DeepSeek と GLM には第一段階で届く。
* Good, because 未対応の機能を送らず、キーの欠落を最初の request より前に示せる。
* Good, because 選んだモデルが会話とともに保存され、要約の送り先もそのモデルに揃う。
* Bad, because 利用者が書いたモデルでは `features` と `context_window` を自分で書く必要がある。誤った値は API のエラーとして現れる。
* Bad, because 切り替えのたびに、履歴にある thinking が失われる。
* Neutral, because `COMPACT_MODEL` を使っていた利用者は `models.toml` へ移す必要がある。

### Confirmation

core の単体テストで、設定の解析と検証（不明な wire、http の base_url、存在しない provider や compact_model）、起動時の選択の優先順位、`features` ごとの request body、bearer と x-api-key のヘッダー、切り替え時の thinking の除去と保存、compaction の送り先を確認する。TUI は `/model` の一覧と切り替えを TestBackend で確認する。実 API での接続は明示的に実行し、通常 CI では行わない。

## Pros and Cons of the Options

### wire と endpoint を分けた registry

* Good, because 三つの wire で主要な接続口を覆え、接続先は設定で増える。
* Good, because 接続先ごとの機能差を `features` で明示できる。
* Bad, because 設定の項目と検証が増える。

### Ollama Cloud に一本化する

* Good, because キー一つで多くのオープンウェイトのモデルを試せる。
* Bad, because OpenAI 本家の GPT-5 に届かない。モデル ID が Ollama 独自で、最新版の提供が遅れることがある。送信内容が Ollama を経由する。

### OpenRouter に一本化する

* Good, because GPT-5 を含む多くのモデルに一つの口で届く。
* Bad, because 料金とデータの経路が仲介者に依存し、各社の Anthropic 互換口や Responses API の機能を使えない。

### ベンダーごとの adapter

* Good, because 各社の固有機能を最大限に使える。
* Bad, because 接続先を増やすたびに adapter と試験が増える。多くは同じ wire の差分でしかない。

## More Information

前提となる議論と各社の endpoint は 2026-11-13 時点の公開文書による。DeepSeek: https://api-docs.deepseek.com/guides/anthropic_api 、Z.ai: `https://api.z.ai/api/anthropic`。モデル ID は各社の文書で確かめてから設定する。2026-11-13 の確認では、DeepSeek-V4.1-Flash の ID は `deepseek-flash`（`deepseek-v4.1-flash` は 400 で拒否される）、GLM-5.3 は `glm-5.3`（1M context、最大出力 128K、推論は常に有効）。DeepSeek の Anthropic 互換口は `x-api-key` と Bearer の両方を受け付け、`output_config.effort` に対応し、`thinking` の adaptive を enabled として扱う。Z.ai の Anthropic 互換口は Bearer（`ANTHROPIC_AUTH_TOKEN`）を使う。関連チケット: SOC-014、SOC-015、SOC-016、SOC-041、SOC-042。

### 後の変更

* 選んだ alias の保存先は、[ADR 7](0007-会話をワークスペース内のセッションとして保存し-cと-rで再開する.md) でセッションの snapshot になった。新しいセッションは、直前に保存したセッションの選択を引き継ぐ。
* 切り替え時に履歴から thinking を取り除くのはやめた。[ADR 14](0014-応答ごとに出所を記録しthinkingは読める送り先にだけ送る.md) で、応答ごとに出所を記録し、送るときに送り先が読める thinking だけを残す。
* `features` に `prompt_caching` が加わり、組み込みの Claude モデルは五つを持つ。
