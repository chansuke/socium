---
number: 23
title: models.devのカタログを利用者が実行するコマンドでmodels.dに書き出す
status: proposed
date: 2026-11-20
links:
- target: 6
  kind: amends
- target: 22
  kind: relates-to
- target: 34
  kind: amendedby
---

# models.devのカタログを利用者が実行するコマンドでmodels.dに書き出す

## Context and Problem Statement

OpenCode が扱うモデルを socium でも使いたい。OpenCode は provider とモデルの一覧を [models.dev](https://models.dev) のカタログ (`https://models.dev/api.json`) から得ている。カタログは provider ごとに、キーを入れる環境変数の名前、接続先の URL、AI SDK の package 名を持ち、モデルごとに context と出力の上限、価格、tool と reasoning の対応を持つ。

ADR 6 では、利用者が `models.toml` に provider とモデルを手で書く。数十の provider と数百のモデルを手で写すのは無理があり、価格や廃止の更新にも追いつけない。一方で `base_url` は API キーの送り先を決める。遠隔のカタログを起動のたびに信じると、カタログの改ざんや誤りでキーが別の host に送られる。

OpenCode Zen のように、一つの provider が一つのキーで複数の形式の口を出すものもある。Zen は Claude を Anthropic Messages、GPT を OpenAI Responses、Gemini を generateContent、ほかを OpenAI Chat Completions で出し、口ごとに URL が違う。

## Decision Drivers

* カタログにある provider とモデルを、手で写さずに使える。
* キーの送り先は、利用者の手元にあって読めるファイルだけが決める。
* 取り込みを何度繰り返しても同じ結果になり、利用者が手で書いた設定を壊さない。
* ADR 6 の provider (wire と接続先の組) の形を変えずに済む。

## Considered Options

* 利用者が実行するコマンドでカタログを `models.d/` のファイルに書き出す
* 起動のたびにカタログを取得して registry に加える
* カタログを socium の binary に同梱する
* provider に wire ごとの URL を持たせ、モデルごとに wire を選ばせる

## Decision Outcome

Chosen option: 「コマンドで `models.d/` に書き出す」。カタログを読むのは利用者がコマンドを実行したときだけで、起動時の registry は手元のファイルだけから作る。

### 重ねて読む registry

* registry は、組み込みの定義、`models.d/*.toml` (ファイル名の順)、`models.toml` の順に重ねる。後のファイルにある同じ alias のモデルは前のものを置き換える。
* `models.d/` は `models.toml` と同じ directory に置く。`SOCIUM_MODELS_FILE` を使うときはそのファイルの隣。
* 二つの `models.d/` のファイルが同じ provider の名前を定めたら誤りにし、両方のファイルを示す。`default` は `models.toml` にだけ書ける。
* 検査は重ねた結果に一度だけ行い、誤りにはファイルの名前を付ける。

### 取り込みのコマンド

* `socium models import <provider-id>...` はカタログを取得し、provider ごとに `models.d/<provider-id>.toml` を書き直す。同じカタログからは同じファイルになる。`--from <file>` で保存したカタログを読める。
* 書き出すたびに、ファイルの path、キーを送る URL、モデルの数、取り込まなかったものとその理由を表示する。利用者はこれを見て、ファイルを残すか消すかを決める。
* 引数がなければ、provider の一覧とモデルの数、取り込めるかどうかを表示する。

### カタログから registry への対応

* AI SDK の package と wire、既定の URL の対応は一つの表に置く。`@ai-sdk/anthropic` は `anthropic`、`@ai-sdk/openai` は `openai-responses`、`@ai-sdk/google` は `google` (ADR 22)、`@ai-sdk/openai-compatible` と OpenAI 互換の package は `openai-chat` にする。表にない package (Bedrock、Vertex、Azure、OAuth だけの provider など) と URL のない provider は取り込まない。
* カタログの provider のモデルが複数の package を使うときは、wire ごとに socium の provider を一つずつ作り、同じ `api_key_env` を共有させる。名前は provider 自身の package に `<id>`、ほかに `<id>-<wire>` を使う。ADR 6 の provider の形は変えない。
* alias は `<provider-id>/<model-id>` にする。上限、価格、`tools = false`、`effort` はカタログの値から写す。Anthropic の API にある Claude には組み込みと同じ五つの feature を付ける。廃止されたモデルと、text を出力しないモデルは取り込まない。

### Consequences

* Good, because OpenCode と同じ provider とモデルを、コマンド一つで使える。
* Good, because キーの送り先は、利用者が読める手元のファイルにしか書かれない。
* Good, because 取り込んだファイルを書き直しても `models.toml` は触られず、手で書いた設定が後から勝つ。
* Bad, because カタログの更新は、利用者がコマンドを再び実行するまで反映されない。
* Bad, because 取り込んだモデルの feature はカタログの粗い情報から推すので、実際の API が受け付けないことがある。その場合は `models.toml` で同じ alias を書き直す。

### Confirmation

core の単体テストで、重ねる順と置き換え、二つのファイルにある provider、`models.d/` の `default`、一つのファイル内の重複した alias の拒否を確かめる。取り込みは、切り詰めたカタログの fixture から、wire ごとの provider への分割とキーの共有、Anthropic の URL の `/v1` の除去、取り込まない package と廃止されたモデル、価格と上限と feature の写しを確かめ、二度の書き出しが同じファイルになることを確かめる。

## Pros and Cons of the Options

### コマンドで `models.d/` に書き出す

* Good, because キーの送り先を利用者が確かめてから使える。
* Bad, because カタログの更新に手で追いつく必要がある。

### 起動のたびにカタログを取得する

* Good, because 常に最新のカタログを使える。
* Bad, because 遠隔のカタログの内容でキーの送り先が変わる。
* Bad, because 起動がネットワークに依存し、遅くなる。

### カタログを binary に同梱する

* Good, because ネットワークが要らない。
* Bad, because 更新に socium の release が要り、価格と廃止が古くなる。
* Bad, because 使わない数百のモデルが `/model` の一覧に並ぶ。

### provider に wire ごとの URL を持たせる

* Good, because OpenCode Zen を一つの provider として書ける。
* Bad, because ADR 6 の provider の形と、それに依る thinking の送り先の判定 (ADR 14) を変えることになる。wire ごとに provider を分ければ同じことが今の形でできる。

## More Information

[ADR 6](0006-モデルを接続形式と接続先の組で登録しmodelコマンドで切り替える.md) の registry を重ねて読むように改める。generateContent の wire は [ADR 22](0022-geminiにはantigravityのログインでなくgemini-apiのキーで接続する.md) による。カタログの形は [models.dev](https://github.com/sst/models.dev) による。
