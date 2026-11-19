---
number: 23
title: サブエージェントに種類のtoolsで認めたMCPのtoolを渡す
status: proposed
date: 2026-11-19
links:
- target: 20
  kind: amends
- target: 9
  kind: amends
- target: 24
  kind: amendedby
---

# サブエージェントに種類のtoolsで認めたMCPのtoolを渡す

## Context and Problem Statement

ADR 20 は MCP の tool をメインのエージェントだけに渡し、サブエージェントでの利用を見送った。Claude Code のサブエージェントは、`tools` を省けばメインと同じ MCP の tool を持ち、`tools` に MCP の tool を名前で書ける。socium でも、issue の調査や外部の API の確認のように MCP の tool を使う作業を、子に任せたい。どの種類にどの MCP の tool を渡し、承認と並行実行をどう扱うかを決める。

## Decision Drivers

* 読み取り専用の種類 (`explorer`、`tools` を省いた利用者の種類) の保証を崩さない。MCP の tool は何をするか socium から分からない。
* 子の MCP の呼び出しも、メインと同じ承認の規則で扱う (ADR 13、ADR 20)。
* 利用者の定義は起動時に読み、MCP サーバーはその後に接続する。定義の検証がサーバーの起動を待ってはならない。
* 新しい設定項目を増やさない。

## Considered Options

* 種類の tool の集合 (`ToolSet`) で決め、利用者の種類は `tools` に MCP の tool を書く
* 種類ごとに MCP を使うかどうかの項目を足す
* 今のまま、メインのエージェントだけに渡す

## Decision Outcome

Chosen option: 「種類の tool の集合 (`ToolSet`) で決め、利用者の種類は `tools` に MCP の tool を書く」。既存の `tools` の意味を広げるだけで、組み込みの種類の性質から渡す範囲が決まる。

* 読み取り専用の種類 (`ToolSet::ReadOnly`: `explorer` と `tools` を省いた利用者の種類) には MCP の tool を渡さない。`worker` (`ToolSet::AllButAgent`) には、メインのエージェントに渡しているすべての MCP の tool を渡す。
* 利用者の種類の `tools` には、`mcp__<server>__<tool>` で一つの tool を、`mcp__<server>__*` でそのサーバーのすべての tool を書ける。定義を読む時点ではサーバーがまだ接続していないので、形だけを確かめる: 接頭辞、設定と同じ規則のサーバー名、続いて tool 名 (64 文字以内の名前の文字) か `*`。サーバーや tool が実在するかは確かめない。`mcp__<server>__*` は `mcp__<server>__` で始まる名前に一致する。サーバー名は `_` で終われない (ADR 20) ので、この接頭辞が別のサーバーの tool に一致することはない。
* 実行時に子へ渡すのは、メインのエージェントに渡している tool のうち、種類が認めるもの。種類が名前を挙げても、どのサーバーも提供していない tool は黙って渡さず、子はそれなしで動く。種類に別のモデルがあっても、そのモデルの tool の一覧に同じ MCP の tool を足す。
* 承認は、メインと同じ `run_tool_call_checkpointed` と同じ `TaskControl` を通す。`allow` に無い tool は尋ね、セッション中の許可は子とメインの両方を覆う。承認の要求には、コマンドと同じく、どのサブエージェントが求めたか (`requested_by`) が付く。答える人がいない実行では拒否し、`allow` に足す方法を伝える。
* MCP の tool を一つでも持つ種類は、ADR 10 の「workspace を変えられる種類」に数える。同じ応答に並んでも他の呼び出しと同時には動かさず、途中で止まったときは変更が残りうることを書き添える。MCP の tool の名前は読み取り専用の一覧に無いので、既存の判定 (`may_change_workspace`) がそのまま当てはまる。
* 再開した子の種類が挙げる MCP の tool が今は無ければ、それなしで動く。記録の中の過去の呼び出しと結果はそのまま残す。

### Consequences

* Good, because MCP の tool を使う調査や作業を子に任せ、メインの文脈を節約できる。
* Good, because 読み取り専用の種類は、MCP が増えても読み取り専用のままになる。
* Good, because 子の MCP の呼び出しも、メインと同じ承認の画面と許可で扱える。
* Bad, because `tools` の MCP の名前を書き誤っても、起動時には分からない。サーバーの接続後も警告は出ず、子にその tool が渡らないだけになる。
* Bad, because MCP の tool を持つ種類は、読み取りしかしない tool でも並行に動かない。

### Confirmation

`catalog.rs` のテストで、`mcp__<server>__<tool>` と `mcp__<server>__*` の読み取りと照合、形の誤り (接頭辞だけ、サーバー名の誤り、`_` で終わるサーバー名、空や不正な tool 名、64 文字超) の文言、`mcp__<server>__*` が `_` で始まる tool に一致し別のサーバーに一致しないこと、MCP の tool を持つ種類が workspace を変えうると判定されること、`explorer` が MCP の tool を認めず `worker` が認めることを確かめる。サブエージェントのテストで、偽の MCP サーバー (`crates/core/tests/fixtures/fake_mcp_server.py`) を使い、`worker` にメインと同じ MCP の tool が渡り `explorer` には渡らないこと、`mcp__fake__*` の種類が `mcp__fake__echo` を呼び結果が次の要求に入ること、提供されていない tool を挙げた種類も起動すること、別のモデルの種類にも渡ること、MCP が無くなった後の再開がそれなしで動くこと、`allow` に無い呼び出しがサブエージェントの名前を付けて尋ね、セッション中の許可がメインの呼び出しも覆うこと、答える人がいない実行で設定の案内とともに拒否されること、MCP の tool を持つ種類の呼び出しが同時に動かないことを確かめる。

## Pros and Cons of the Options

### 種類の tool の集合 (`ToolSet`) で決め、利用者の種類は `tools` に MCP の tool を書く

* Good, because 設定項目が増えず、Claude Code の `tools` の書き方に近い。
* Bad, because MCP の名前の誤りは実行時まで分からない。

### 種類ごとに MCP を使うかどうかの項目を足す

* Good, because MCP を使う種類が一目で分かる。
* Bad, because `tools` と二重の指定になり、両方の組み合わせの意味を決める必要がある。

### 今のまま、メインのエージェントだけに渡す

* Good, because 承認と並行実行の扱いを考えずに済む。
* Bad, because MCP の tool を使う作業は、すべてメインの文脈で行うしかない。

## More Information

[ADR 20](0020-mcpサーバーを利用者の設定だけからstdioで起動しtoolを既定で尋ねて呼ぶ.md) の「MCP の tool はメインのエージェントだけに渡す」を改め、[ADR 9](0009-利用者定義のサブエージェントを設定ディレクトリのmarkdownで定める.md) の `tools` に MCP の tool の名前を足す。並行実行の規則は [ADR 10](0010-サブエージェントのモデル選択と並行実行と記録と再開.md)、承認は [ADR 13](0013-承認を取り消しにくい操作に絞りセッション単位の許可を持つ.md)。Claude Code のサブエージェントの `tools` は [Claude Code の文書](https://code.claude.com/docs/en/sub-agents)。
