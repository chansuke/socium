---
number: 35
title: socium -pは3秒何も届かないstdinを読まずに警告して進む
status: proposed
date: 2026-11-27
links:
- target: 19
  kind: amends
---

# socium -pは3秒何も届かないstdinを読まずに警告して進む

## Context and Problem Statement

ADR 19 では、`socium -p` は stdin が端末でなければ EOF まで読んでから依頼を作る。ところが、エージェントのシェル、CI のランナー、`ssh` の非対話実行は、閉じない socket やパイプを stdin として渡すことがある。Claude Code の Bash ツールから `socium -p "..."` を実行すると、stdin は閉じない socket になる。socium は EOF を待ち続け、何も出力せずに止まった。ADR 19 は stdin の扱いを `claude -p` に揃える方針である。手元の Claude Code 2.1.329 は、stdin に 3 秒何も届かなければ警告を出し、stdin なしで進む。`-p` が黙った stdin をどこまで待つかを決める。

## Decision Drivers

* エージェントや CI から `-p` を呼んでも、stdin を閉じ忘れただけで止まらない。
* `git diff | socium -p "review this"` のようなパイプの入力は、これまでどおり全部読む。
* 待つのをやめたときは、そのことと避け方を利用者に伝える。
* `claude -p` を使い慣れた人が同じ挙動を期待できる。

## Considered Options

* 最初の 1 バイトか EOF を 3 秒待ち、どちらも来なければ警告して stdin なしで進む
* 今までどおり EOF まで待つ
* stdin が socket のときだけ読まない

## Decision Outcome

Chosen option: 「最初の 1 バイトか EOF を 3 秒待ち、どちらも来なければ警告して stdin なしで進む」。閉じない stdin で止まらず、`claude -p` とも揃う。

* stdin が端末でなければ、別のスレッドで読む。3 秒以内に 1 バイトか EOF が届けば、それ以降は EOF まで期限なしで読み、ADR 19 のとおり依頼に加える。
* 3 秒以内にどちらも届かなければ、stdin を読まずに進む。stderr には、stdin を読まなかったことと、`< /dev/null` で待たずに済むことを `socium: ` を付けて出す。プロンプトもなければ、ADR 19 のとおり使い方の誤り (2) で終える。
* 読みかけのスレッドは止めずに残す。turn が終わればプロセスごと終わる。

### Consequences

* Good, because エージェントや CI から呼んでも、stdin のせいで止まらない。
* Good, because 最初の 1 バイトが届いてからは期限がないので、途中で間が空くパイプも全部読める。
* Bad, because 最初の出力まで 3 秒を超えるコマンドをパイプでつなぐと、その出力は読まれない。stderr の警告を見て、出力をファイルに保存してから渡す必要がある。
* Bad, because stdin が閉じない呼び出しでは、毎回 3 秒待つ。`< /dev/null` を付ければ待たない。

### Confirmation

TUI の crate の統合テストで、実際のバイナリを起動して次のことを確かめる。
* 開いたまま何も書かない stdin を渡しても、プロンプトだけで turn が終わる。stderr には `< /dev/null` を含む警告が出る。
* 最初の部分を書いてから 3 秒より長く間を空けて残りを書いても、両方が依頼になる。

## Pros and Cons of the Options

### 最初の 1 バイトか EOF を 3 秒待つ

* Good, because 閉じない stdin で止まらず、`claude -p` と同じように振る舞う。
* Bad, because 最初の出力が遅いパイプの入力を落とすことがある。

### EOF まで待つ

* Good, because 遅いパイプの入力も必ず読む。
* Bad, because 閉じない stdin を渡されると、何も出力せずに止まり続ける。

### stdin が socket のときだけ読まない

* Good, because パイプとファイルの入力は今までどおり全部読む。
* Bad, because 閉じないパイプでは同じように止まる。また、socket で渡された入力を黙って捨てる。

## More Information

[ADR 19](0019-socium-pの既定を返答の本文にしstdinと出力形式の選択をclaude-pに揃える.md) を改める。
