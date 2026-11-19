---
number: 20
title: MCPサーバーを利用者の設定だけからstdioで起動しtoolを既定で尋ねて呼ぶ
status: proposed
date: 2026-11-19
links:
  - target: 13
    kind: amends
---

# MCPサーバーを利用者の設定だけからstdioで起動しtoolを既定で尋ねて呼ぶ

## Context and Problem Statement

Claude Code と OpenCode は MCP (Model Context Protocol) のクライアントを持ち、利用者が登録したサーバーの tool をモデルに渡す。GitHub の issue、社内の API、データベースのように、socium の組み込み tool では届かない対象をモデルが扱えるようになる。socium にはこの口が無かった (改善計画の PLAN-11)。

MCP サーバーの多くはネットワークとホストの認証情報を必要とする。socium のサンドボックスはネットワークを止め、ホームディレクトリを隠すので、サーバーをサンドボックスの中では動かせない。サーバーをどこで動かし、誰がそれを決め、モデルがその tool を呼ぶときに何を確かめるかを決める。最初の版は、使える最小の範囲に絞る。

## Decision Drivers

* ワークスペースは信頼しない ([脅威モデル](../threat-model.md))。ワークスペースの中身が、ホストで動くプログラムを決めてはならない。
* モデルの出力は権限を与えない。MCP の tool を呼ぶ判断も、ADR 13 の承認と同じ規則で扱う。
* プロバイダーの API キーを子プロセスに渡さない。
* サーバーが一つ起動に失敗しても、socium は起動する。
* 新しい依存を増やさない。tool の実行はすでに blocking のスレッドで動いている。

## Considered Options

* サーバーをサンドボックスの中で動かす
* サーバーをホストで動かし、設定を `config.toml` だけから読む
* サーバーをホストで動かし、ワークスペースの `.mcp.json` も読む

## Decision Outcome

Chosen option: 「サーバーをホストで動かし、設定を `config.toml` だけから読む」。Claude Code と OpenCode もローカルの MCP サーバーをホストのプロセスとして動かす。サーバーが必要とするネットワークと認証情報は、サンドボックスの中では用意できない。

* 設定は `config.toml` の `[mcp.servers.<name>]` に書く。`command` (argv、必須)、`env` (`${NAME}` は socium の環境から展開する。未設定の変数があればそのサーバーを起動せず警告する)、`allow` (尋ねずに実行する tool 名)、`enabled` (既定は true)。サーバー名は `[A-Za-z0-9_-]{1,32}` で `__` を含まない。ワークスペースの `.mcp.json` のようなプロジェクト単位の設定は読まない。
* 接続は stdio だけにする。サーバーは子プロセスとして起動し、作業ディレクトリはワークスペース、環境は一度空にしてから PATH・HOME・USER・LANG (ホストで設定されていれば) と `env` だけを渡す。ハンドシェイクは `initialize` (protocolVersion `2025-06-18`、サーバーが返した版を受け入れる)、`notifications/initialized`、`nextCursor` をたどる `tools/list`。サーバーごとに 30 秒の期限で並行に起動し、失敗したサーバーは stderr の末尾を添えた警告にして飛ばす。起動は画面が開く間にスレッドで進め、最初の turn の前に待つ。
* tool は `mcp__<server>__<tool>` の名前でモデルに渡す。64 文字を超える名前と、`[A-Za-z0-9_-]` 以外を含む名前は警告して渡さない。説明と `inputSchema` はサーバーのものを使う。
* tool の呼び出しは組み込み tool と同じ経路 (`run_tool_call_checkpointed`) を通す。承認、`tool_started` の記録、中断はそのまま適用される。中断したら `notifications/cancelled` を送る。結果は `text` を連結し、画像などは 1 行の置き換えにし、`structuredContent` しか無ければその JSON を返す。`isError` は tool のエラーにする。
* 承認は ADR 13 に `Grant::McpTool` を足す。`allow` にある tool と、このセッションで許可した tool は尋ねずに実行する。それ以外は「MCP tools run in the <server> server on the host, outside Socium's sandbox.」を理由に尋ね、その tool だけをセッション中許可する選択肢を出す。答える人がいない実行では拒否し、`allow` に足す方法を伝える。
* MCP の tool はメインのエージェントだけに渡す。サブエージェントには渡さない。
* 今回は扱わない: resources、prompts、sampling、roots、HTTP などの遠隔接続、OAuth、`notifications/tools/list_changed`、サブエージェントでの利用。

### Consequences

* Good, because ワークスペースの内容では、ホストで動くプログラムが増えない。サーバーを決めるのは利用者だけになる。
* Good, because プロバイダーの API キーなど socium の環境変数がサーバーに渡らない。
* Good, because 既定では MCP の tool ごとに人が確かめる。尋ねずに動かすのは、利用者が `allow` に書いたものかセッション中に許可したものに限る。
* Bad, because サーバーはサンドボックスの外で動く。サーバーがホストで何をするかは socium の制御の外にある。
* Bad, because サーバーの返す内容はモデルにそのまま渡る。そこに書かれた指示にモデルが従うことがある。
* Neutral, because 遠隔のサーバーとプロジェクト単位の設定が無いので、Claude Code や OpenCode の設定をそのまま持ち込めない。

### Confirmation

`settings.rs` のテストで、`[mcp.servers]` の読み取り、既定値、不正な名前、`${VAR}` の展開と未設定の変数、`enabled = false` を確かめる。`mcp.rs` のテストで、Python の偽のサーバー (`crates/core/tests/fixtures/fake_mcp_server.py`) を子プロセスとして起動し、渡す tool の名前と schema、結果の変換、`isError`、中断がすぐ戻りサーバーに伝わること、呼び出し中に終了したサーバーのエラーに stderr が付くこと、起動に失敗したサーバーを飛ばすこと、registry を捨てると子プロセスが消えること、子プロセスが親の API キーを見ないことを確かめる。`approval.rs` と `tools/mod.rs` のテストで、既定で尋ねること、`allow` の tool は尋ねないこと、セッション中の許可がその tool だけを覆うこと、答える人がいない実行の案内を確かめる。`lib.rs` のテストで、モデルが `mcp__fake__echo` を呼び、その結果が次の要求に入ることを、サブエージェントのテストで worker に MCP の tool が渡らないことを確かめる。

## Pros and Cons of the Options

### サーバーをサンドボックスの中で動かす

* Good, because サーバーの動作がワークスペースの中に閉じる。
* Bad, because ネットワークとホストの認証情報が無いので、多くのサーバーが動かない。サンドボックスに例外を作れば、組み込みのコマンドと同じ保証が崩れる。

### サーバーをホストで動かし、設定を `config.toml` だけから読む

* Good, because ホストで動くものを決めるのは、モデルが書き換えられない設定だけになる (ADR 12)。
* Bad, because プロジェクトごとのサーバーを、利用者が自分の設定に書く手間がある。

### サーバーをホストで動かし、ワークスペースの `.mcp.json` も読む

* Good, because リポジトリと一緒にサーバーの設定を配れる。
* Bad, because 取り込んだリポジトリが、ホストで動くプログラムを決められる。初回に承認を求めても、その判断を利用者に押し付けることになる。

## More Information

MCP の stdio 接続は仕様の [Transports](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports)、tool は [Tools](https://modelcontextprotocol.io/specification/2025-06-18/server/tools)、中断は [Cancellation](https://modelcontextprotocol.io/specification/2025-06-18/basic/utilities/cancellation)。Claude Code の MCP は [Claude Code の文書](https://code.claude.com/docs/en/mcp)、OpenCode は [OpenCode の文書](https://opencode.ai/docs/mcp-servers/)。承認の規則は [ADR 13](0013-承認を取り消しにくい操作に絞りセッション単位の許可を持つ.md)、設定をワークスペースの外に置く理由は [ADR 12](0012-サンドボックスを広げる口をモデルから書けない設定ファイルに限る.md)。
