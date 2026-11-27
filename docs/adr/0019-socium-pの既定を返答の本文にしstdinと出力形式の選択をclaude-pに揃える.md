---
number: 19
title: socium -pの既定を返答の本文にしstdinと出力形式の選択をclaude -pに揃える
status: proposed
date: 2026-11-19
links:
- target: 17
  kind: amends
- target: 35
  kind: amendedby
---

# socium -pの既定を返答の本文にしstdinと出力形式の選択をclaude -pに揃える

## Context and Problem Statement

ADR 17 の `socium -p PROMPT` は、いつも JSON Lines を stdout に出す。評価には足りるが、シェルで返答だけを使うには JSON を読み解く必要があり、`git diff | socium -p "review this"` のようにパイプの入力を依頼に添えることもできない。`claude -p` は、既定で返答の本文だけを出し、プロンプトを位置引数で受け取り、パイプの stdin を読み、`--output-format text|json|stream-json` で形式を選ぶ ([CLI reference](https://code.claude.com/docs/en/cli-reference))。`-p` の入出力をどこまで `claude -p` に揃えるかを決める。

## Decision Drivers

* シェルで返答をそのまま次のコマンドに渡せる。
* `claude -p` を使い慣れた人が同じ書き方で使える。
* 評価や自動化が今の JSON Lines を同じ形で取れる。
* 終了コードと、取り消しの扱いを変えない。

## Considered Options

* 既定を返答の本文にし、`--output-format` で JSON と JSON Lines を選べるようにする
* 既定を JSON Lines のままにし、`--output-format text` を足す
* 今の形を保つ

## Decision Outcome

Chosen option: 「既定を返答の本文にし、`--output-format` で選べるようにする」。人がシェルで使うときの既定が短くなり、評価は形式を 1 つ指定するだけで今と同じ記録を取れる。

* `-p` (`--print`) は値を取らないフラグにする。プロンプトは位置引数 1 つで、`-p` の前でも後ろでもよい。2 つ目の位置引数と、`-p` なしの位置引数は誤り。ADR 17 の `-p -` はやめ、`-` は誤った引数になる。`-r` の選択画面と組み合わせられないことは変えない。
* stdin が端末でないときは読む。stdin とプロンプトの両方があれば、stdin の本文、空行、プロンプトの順に 1 つの依頼にする。stdin の末尾の改行は落とす。どちらも空なら使い方の誤り (2) で終える。
* `--output-format` は `text` (既定)、`json`、`stream-json` を受け付け、`-p` なしでは誤り。
  * `text`: 成功したら返答の本文と改行だけを stdout に出す。失敗の内容と notices は stderr に `socium: ` を付けて出す。
  * `json`: ADR 17 の `finished` の記録 1 行だけを出す。
  * `stream-json`: ADR 17 の JSON Lines をそのまま出す。
* 終了コード、承認の拒否、Ctrl+C と stdout への書き込みの失敗での取り消しは ADR 17 のまま。

### Consequences

* Good, because `socium -p "summarize" > file` やパイプで、返答をそのまま使える。
* Good, because 評価の `evals/run.py` は `--output-format stream-json` を足すだけで今の記録を読める。
* Bad, because ADR 17 の既定の出力を前提にした呼び出しは、`--output-format stream-json` を足すまで JSON を受け取れない。
* Bad, because `text` では経過が見えず、途中で止まったときに何をしていたかは stderr の診断とセッションの activity log で見るしかない。

### Confirmation

TUI の crate の統合テストで、実際のバイナリを環境を空にしてローカルのサーバーに対して起動し、既定では stdout が返答と改行だけであること、パイプの stdin とプロンプトが空行を挟んでモデルへの依頼になること、stdin だけでも依頼になること、どちらも空なら 2 で終わること、`--output-format json` で `finished` の 1 行だけが出ること、`text` で失敗すると 1 で終わり stderr にエラーが出ることを確かめる。ADR 17 のテストは `--output-format stream-json` を付けて同じことを確かめる。引数の解釈は `cli.rs` の単体テストで確かめる。

## Pros and Cons of the Options

### 既定を返答の本文にし、`--output-format` で選ぶ

* Good, because `claude -p` と同じ書き方で使える。
* Bad, because ADR 17 の既定の出力と互換がない。

### 既定を JSON Lines のままにし、`--output-format text` を足す

* Good, because 今の呼び出しを変えずに済む。
* Bad, because シェルで使うたびにフラグが要り、`claude -p` と既定が食い違う。

### 今の形を保つ

* Good, because 何も変えずに済む。
* Bad, because 返答だけを取るには JSON を読み解く道具が要り、パイプの入力を添えられない。

## More Information

[ADR 17](0017-tuiなしで一つのturnを実行しjson-linesで報告する.md) を改める。`claude -p` の振る舞いは [CLI reference](https://code.claude.com/docs/en/cli-reference) による。
