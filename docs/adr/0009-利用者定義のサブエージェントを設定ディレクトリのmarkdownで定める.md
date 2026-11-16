---
number: 9
title: 利用者定義のサブエージェントを設定ディレクトリのMarkdownで定める
status: accepted
date: 2026-11-14
---

# 利用者定義のサブエージェントを設定ディレクトリのMarkdownで定める

## Context and Problem Statement

ADR 8 で `agent` tool と、組み込みの二種類のサブエージェント（読み取りだけの `explorer` と、`agent` 以外のすべての tool を持つ `worker`）を入れた。用途ごとに、子の system prompt、使える tool、モデルを利用者が決めたい。たとえば、差分のレビューだけをする読み取り専用の種類や、調査を安いモデルに任せる種類である。

Claude Code は `.claude/agents/*.md`（プロジェクト）と `~/.claude/agents/*.md`（利用者）に、YAML の front matter（`name`、`description`、`tools`、`model`）と、system prompt にあたる本文を書く。Codex CLI は設定ファイルの `[agents]` に役割を書き、`spawn_agent` の `agent_type` で選ぶ。

## Decision Drivers

* 長い system prompt を読みやすく書け、ほかの環境と共有しやすい。
* 定義が組み込みの種類の安全性を崩さない。tool の一覧は `agent` tool と同じ仕組みで強制し、入れ子を許さない。
* 読み込んだリポジトリの内容が、利用者の知らない種類を足せない。
* 定義の誤りで起動が止まらない。誤りは、どのファイルの何が悪いかを示す。
* 依存を増やさない。

## Considered Options

* 利用者の設定ディレクトリに、TOML の front matter を持つ Markdown を置く
* `models.toml` に `[agents]` の表を足す（Codex CLI の型）
* プロジェクトのディレクトリにも置けるようにする（Claude Code の `.claude/agents` の型）
* Claude Code と同じ YAML の front matter にする

## Decision Outcome

Chosen option: 「利用者の設定ディレクトリに、TOML の front matter を持つ Markdown を置く」。長い system prompt を本文にそのまま書け、一つの種類が一つのファイルになるので共有しやすい。front matter は既存の依存の `toml` で読める。

### 場所

`SOCIUM_AGENTS_DIR` で指定したディレクトリ、無ければ `$XDG_CONFIG_HOME/socium/agents/`（未設定なら `~/.config/socium/agents/`）の `*.md` を、ファイル名の順に読む。起動時に一度だけ読み、変更は再起動で反映する。

プロジェクトのディレクトリからは読まない。リポジトリに置かれた定義は、clone した人が中身を読まないまま、親のモデルへの説明と子の system prompt に入る。承認と sandbox は残るが、`description` で親のモデルを誘導し、`tools` の広い種類を選ばせることができる。プロジェクト単位の指示の扱い（出典、scope、信頼の区別）は SOC-022 で決めてから考える。

### 形式

```markdown
+++
name = "reviewer"
description = "Reviews uncommitted changes for correctness bugs"
tools = ["read_file", "search_text", "git_diff"]
model = "sonnet"
+++
You are a code reviewer ...
```

* `name`（必須）: 英小文字で始まり、`a-z`、`0-9`、`-`、`_` からなる 32 文字以内。組み込みの `explorer` と `worker` は使えない。`agent` tool の `agent_type` の値になる。
* `description`（必須）: 300 文字以内の一行。`agent` tool の説明に種類の一覧として載り、親のモデルが種類を選ぶ手がかりになる。
* `tools`（任意）: 組み込みの tool の名前の一覧。省くと `explorer` と同じ読み取り専用の tool になる。最小の権限を既定にするため。`agent` と、存在しない名前は誤り。
* `model`（任意）: registry の alias。省くと、呼び出した時点の親のモデルを使う。
* 本文: 前後の空白を除いて、子の system prompt にする（後の変更を参照）。空なら誤り。
* 知らない項目は誤りとする。綴りの誤りで設定が黙って無視されないようにするため。

### 検証と誤り

* 誤りのあるファイル、64 KiB を超えるファイル、同じ `name` の二つ目以降、33 個目以降は、ファイル名と理由を示す起動時の警告を出して読み飛ばす。起動は止めない。
* `model` が registry に無い、またはその provider の API キーが無い種類も、警告して読み飛ばす。親のモデルに黙って切り替えると、利用者の意図しない送り先と費用になるため。
* `SOCIUM_AGENTS_DIR` を指定したのにディレクトリが読めなければ警告する。既定の場所が無いのは、定義が無いという意味で、警告しない。

### 実行

ADR 8 の実行の仕組みをそのまま使う。

* `agent` tool の `agent_type` の列挙と説明に、組み込みの種類の後に利用者の種類を並べる。`/model` で親のモデルを変えても、種類の一覧は保つ。
* 子に渡す tool は、子のモデルに渡す tool の一覧（native の text editor の有無を含む）を、種類の `tools` で絞ったもの。tool loop は渡していない tool の呼び出しを拒否するので、一覧の外は実行されない。
* `model` を持つ種類は、そのモデルの provider、ID、context window、最大出力（`max_output_tokens`、無ければ `MAX_TOKENS`、無ければ既定値）で動く。
* 読み取り専用の一覧の外の tool を一つでも持つ種類が途中で止まった場合は、`worker` と同じく変更が残っていることを書き添える。

### Consequences

* Good, because 用途ごとの system prompt、tool、モデルを、コードを変えずに足せる。
* Good, because 種類を足しても、tool の強制、入れ子の禁止、承認は組み込みの種類と変わらない。
* Good, because 調査やレビューを安いモデルに任せられる。
* Bad, because Claude Code の定義ファイル（YAML の front matter）をそのままは使えない。front matter を TOML に書き換える必要がある。
* Bad, because プロジェクトごとの種類を共有できない。
* Neutral, because 定義の変更は再起動まで反映しない。

### Confirmation

core の単体テストで、front matter と本文の解析、`tools` の既定と書き込む tool の判定、各種の誤りの文言、ファイル名の順の読み込みと誤ったファイルの読み飛ばし、名前の重複、未登録や起動できないモデルの種類を読み飛ばすこと、明示したディレクトリが無いときだけ警告すること、大きすぎるファイル、`agent` tool への種類の列挙、例のファイル（`agent.md.example`）が読めることを確かめる。`ScriptedProvider` で、利用者の種類が自分の system prompt、tool、モデル、最大出力で動き、親のモデルには子の round が送られないことを確かめる。起動の処理で定義が読まれ、モデルの切り替え後も種類が残ることを確かめる。

## Pros and Cons of the Options

### 設定ディレクトリの Markdown（TOML の front matter）

* Good, because 長い prompt を引用符やエスケープなしで書ける。
* Good, because ファイル単位で共有、追加、削除できる。
* Bad, because ファイルが増え、設定が `models.toml` と分かれる。

### `models.toml` の `[agents]`

* Good, because 設定が一つのファイルにまとまり、モデルの alias との対応を同じ場所で検証できる。
* Bad, because 長い system prompt を TOML の複数行文字列に書くことになり、読みにくい。
* Bad, because `models.toml` の誤りは起動を止める。種類一つの誤りで起動できなくなる。

### プロジェクトのディレクトリにも置く

* Good, because チームでリポジトリごとに種類を共有できる。
* Bad, because リポジトリの内容が、利用者の確かめていない指示を親と子のモデルに入れる。信頼の区別が SOC-022 で決まっていない。

### YAML の front matter

* Good, because Claude Code の定義をそのまま使える。
* Bad, because YAML の parser の依存が増える。広く使われてきた `serde_yaml` は保守が終わっている。

## More Information

Claude Code の定義の形式は公開文書（https://docs.anthropic.com/en/docs/claude-code/sub-agents ）による。関連: ADR 6（モデルの registry）、ADR 8（`agent` tool）、SOC-022（プロジェクト単位の指示）。

### 後の変更

2026-11-16: 本文は子の system prompt のすべてではなく、組み込みの種類と共通の前置きの後に置く指示になった。前置きは、子が利用者の workspace で動くこと、委ねた側の会話が見えないこと、最後の message だけが返ることを伝える。本文がそのまま system prompt になると、利用者が書き忘れた種類の子は、自分が委任を受けた子であることも、報告が最後の message だけであることも知らずに動いていた。組み込みの `explorer` と `worker` も、同じ前置きの後に種類ごとの指示を置く形に分けた。本文が空なら誤りとするのは変わらない。
