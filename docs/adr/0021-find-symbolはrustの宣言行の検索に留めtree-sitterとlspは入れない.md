---
number: 21
title: find_symbolはRustの宣言行の検索に留めtree-sitterとLSPは入れない
status: proposed
date: 2026-11-20
---

# find_symbolはRustの宣言行の検索に留めtree-sitterとLSPは入れない

## Context and Problem Statement

`find_symbol` は、ripgrep で `.rs` ファイルだけを探し、`fn`、`struct`、`enum`、`trait`、`type`、`mod`、`const`、`static` に名前が続く行と、名前を含む `impl` の行を返す。構文を解析しないので、コメントや文字列の中の同じ並びも返し、複数行に分かれた宣言を見落とし、別のモジュールにある同名の項目を区別できず、参照は探せない。一方でツールの名前は言語を限らず、説明は Rust に限ると書いていたが、ほかの言語を探さないことは書いていなかった。tree-sitter や LSP で言語ごとの解析に置き換えるか、今の検索に留めてその限界を明示するかを決める。

## Decision Drivers

* モデルが、探していない言語の結果を「宣言がない」と読み違えない。
* 置き換えるなら、実際の作業で精度か時間が良くなることを測って示せる。
* 依存と配布物を、使われない機能のために増やさない。

## Considered Options

* 今の検索に留め、対象と限界を説明と空の結果に書く
* tree-sitter で言語ごとに宣言を解析する
* LSP サーバーを起動して定義と参照を問い合わせる

## Decision Outcome

Chosen option: 「今の検索に留め、対象と限界を説明と空の結果に書く」。手元の評価では、モデルはほとんど `find_symbol` を使わず、`search_text` と `find_symbol` を外しても合格数は変わらなかったので、解析器を入れて良くなる作業を示せない。

* 説明に、`.rs` だけを探すこと、ほかの言語には `search_text` を使うこと、行ごとの一致であることとその限界、参照を探さないことを書く。
* 一致がないときは、結果に「Rust (.rs) ファイルだけを読む。ほかの言語には search_text を使う」と添える。
* 名前は変えない。保存済みの会話、ツールのプロファイル、評価の記録が `find_symbol` を参照している。

根拠にした測定は次のとおり。どちらも 39 タスクを各 3 回実行した。

* claude-haiku-5-5 で prompt cache の有無を比べた 234 回 (chansuke/socium#26) で、ツール呼び出し 1,758 回のうち `find_symbol` は 1 回、`search_text` は 161 回だった。
* ds-flash でツールの組を比べた測定 (chansuke/socium#38) で、全ツールの `full` は 114/117、`search_text` と `find_symbol` を外した `basic` は 115/117、検索だけを足した `search` は 116/117 が合格し、95% Wilson 区間で差はなかった。

評価のタスクはほとんどが Python で、Rust は 3 ファイルしかない。大きな Rust のコードベースで宣言を探す作業が多ければ結果は変わりうるが、それは測っていない。

### Consequences

* Good, because 依存も配布物も増えず、モデルは空の結果からほかの言語では `search_text` を使えばよいと分かる。
* Good, because 限界をテストに固定したので、変えるときは判断として変わる。
* Bad, because Rust でも、コメントや文字列の誤検出、複数行の宣言の見落とし、同名の項目の区別、参照の検索は解決しない。
* Bad, because 名前が言語を限らないまま残り、説明を読まないと Rust 専用だと分からない。

### Confirmation

`crates/core/src/tools/find_symbol.rs` の単体テストで、コメントと文字列の中の宣言らしい行を返し、別のモジュールの同名の `build` を両方返し、2 行に分かれた宣言を返さないことを固定する。Python と TypeScript だけのワークスペースでは一致がなく、結果に Rust だけを読むことと `search_text` が書かれることを確かめる。解析器を入れるかを見直すときは、`evals/run.py` に宣言や参照を探すタスクを足し、`--tools` で今の検索と比べて精度と時間の差を測る。

## Pros and Cons of the Options

### 今の検索に留め、対象と限界を書く

* Good, because 変更が説明と空の結果の注記だけで済み、ripgrep 以外の依存が要らない。
* Bad, because Rust でも行ごとの一致の限界が残る。

### tree-sitter で言語ごとに解析する

* Good, because コメントと文字列を除き、複数行の宣言も見つけられる。
* Bad, because 言語ごとの文法を持ち込み、配布物が大きくなる。測った作業では使われておらず、良くなる根拠がない。

### LSP サーバーを起動して問い合わせる

* Good, because 定義と参照を型に基づいて引ける。
* Bad, because 言語ごとのサーバーと、その起動時間とサンドボックスの中での動作が要り、どれも配布物と設定に入らない。

## More Information

元のチケットは SOC-025 (chansuke/socium#30)。ツールのプロファイルは `docs/configuration.md` の Tool profiles を参照。
