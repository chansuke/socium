---
number: 30
title: 遠隔のMCPサーバーにStreamable HTTPとヘッダーだけで接続する
status: proposed
date: 2026-11-19
links:
- target: 29
  kind: amends
- target: 31
  kind: amendedby
---

# 遠隔のMCPサーバーにStreamable HTTPとヘッダーだけで接続する

## Context and Problem Statement

ADR 29 は MCP サーバーを stdio の子プロセスに限り、HTTP などの遠隔接続と OAuth を見送った。GitHub の公式サーバー (`https://api.githubcopilot.com/mcp/`) や DeepWiki (`https://mcp.deepwiki.com/mcp`) のように、ホストで動かすプログラムを持たず URL だけで提供されるサーバーが増えている。Claude Code と OpenCode は、どちらも URL とヘッダーで遠隔のサーバーを登録できる。socium から遠隔のサーバーに、どの接続方式で、どの認証で、どのスレッド構成でつなぐかを決める。

## Decision Drivers

* 設定は ADR 29 と同じく `config.toml` だけから読む。ワークスペースは遠隔のサーバーを足せない。
* トークンを設定ファイルに書かずに済み、警告、エラー、起動時の表示、activity、system prompt に値が出ない。
* 鍵を送る先の規則 (https、平文の http は loopback だけ) をモデルのプロバイダーと揃える。
* クライアントの API は同期のまま保つ。tool の実行は blocking のスレッドで動き、起動は専用のスレッドで動く。`AgentService` は TUI の tokio runtime の中で捨てられるので、そこで runtime や `reqwest::blocking` の client を捨てて panic してはならない。
* 新しい依存を増やさない。reqwest (async) と tokio はすでにある。

## Considered Options

* Streamable HTTP だけを、設定したヘッダーで使う
* Streamable HTTP に加えて、2024-11-05 の HTTP+SSE にも落ちる
* OAuth まで実装する

## Decision Outcome

Chosen option: 「Streamable HTTP だけを、設定したヘッダーで使う」。2025-06-18 の仕様で HTTP+SSE は置き換えられており、主要な遠隔サーバーは Streamable HTTP を提供している。トークンをヘッダーで渡せば、OAuth を持たないまま GitHub のような認証の要るサーバーにも届く。

* `[mcp.servers.<name>]` は `command` (stdio) と `url` (HTTP) のどちらか一方だけを持つ。両方、またはどちらも無ければ誤り。`url` には任意で `headers` を書け、値の `${NAME}` は `env` と同じく socium の環境から展開する (未設定ならそのサーバーを起動せず警告する)。`url` に `env`、`command` に `headers` を書くと誤り。設定の内部では `McpTransportSettings::Stdio { command, env } | Http { url, headers }` の enum にする。
* `url` は書かれたまま使い、プロバイダーの `base_url` と同じ規則 (`models::check_endpoint_url`) で確かめる。https、平文の http は loopback だけ、資格情報・query・fragment は不可。ヘッダー名は HTTP のヘッダー名でなければならず、socium が自分で付ける `Accept`、`Content-Type`、`Content-Length`、`Host`、`Mcp-Session-Id`、`MCP-Protocol-Version` は書けない。展開後の値がヘッダーの値として不正なら、値を示さずに誤りにする。
* ヘッダーの値は socium からは出さない。起動時の表示は URL とヘッダー名だけ、`Debug` も名前だけ、HTTP client では sensitive を付ける。エラーは状態コードとサーバーの JSON-RPC エラーの文だけを引用する。サーバーがその文にトークンを書き戻すこともあるので、警告とエラー (HTTP のエラー、JSON-RPC のエラー、tool のエラーの結果、起動の失敗) では、ヘッダーの値と、その最後の語 (`Bearer <token>` の token) を `[redacted]` に置き換える (ADR 29 と同じく 8 文字以上のもの)。引用を切り詰めるのは置き換えた後で、切れ目にトークンの一部が残らない。サーバーが変形して書き戻したものまでは防げない。`config.toml` の誤りは、行と列とキーの名前と、値を `"…"` に伏せた説明だけで示す。TOML の既定の表示は該当の行を引用するので、ファイルに直接書いたトークンが起動時の誤りに出てしまうため。
* プロキシは環境の設定に従う。ただし平文の http を許す loopback の URL ではプロキシを使わない。環境の `HTTP_PROXY` に従うと、ヘッダーが平文でプロキシに渡るため。
* 接続は仕様の Streamable HTTP に従う。JSON-RPC のメッセージごとに URL へ POST し、`Accept: application/json, text/event-stream` と `Content-Type: application/json` を付ける。request の応答は JSON の本文か SSE で、`Content-Type` の media type を parameter を除き大小文字を区別せずに `application/json` か `text/event-stream` と完全に比べる (`application/json-patch+json` などは誤り)。SSE からは同じ id の応答を取り、途中のサーバーの request には stdio と同じく ping だけに答え (POST で返す)、通知は読み捨てる。答えの POST は同時に 8 本までにし、それを超えた分は捨てる。通知と答えの POST は 10 秒で諦める。SSE は WHATWG の解析に従い、CRLF、LF、CR のどれも行末として扱い、先頭の BOM を飛ばす。イベントと行の上限は、受け取る前に確かめる。一つのチャンクの中で完了したイベントは、同じチャンクの後ろにある上限を超える行より先に扱うので、応答が届いていればチャンクの切れ目によらず受け取る。通知と応答の POST は 202 を受け、それ以外の状態は誤りにする。initialize の応答の `Mcp-Session-Id` を以後のすべての要求に付け、initialize の後は合意した版を `MCP-Protocol-Version` で送る。redirect はたどらない。
* セッションの 404 では、仕様どおりセッション ID と版を捨て、新しい initialize でセッションを始め直す。セッションには世代を数え、404 で捨てるのは、その要求を送ったときの世代が今の世代のときだけにする。前のセッションで送った要求への遅れた 404 が、新しいセッションを消さないため。404 を返された `tools/call` はサーバーが実行していないが、自動では送り直さず、「新しいセッションを始めたので呼び直すこと」を tool のエラーとしてモデルに返す。通知や ping への答えが 404 を受けてセッションが終わっていたときは、次の呼び出しを送る前にセッションを始め直す。同時の呼び出しが始め直すのは一度だけにする。tool の一覧は取り直さない。新しいセッションで一覧が変わっても、起動時の一覧のまま offer し、無くなった tool はサーバーのエラーになる。変わった一覧は設定を読み直すか再起動したときに反映する。401 は、設定の `Authorization` が拒まれたこと、または (ADR 31) ログインが要ることを伝えるエラーにする。サーバーを捨てるときはセッション ID を付けた DELETE を送り、2 秒で諦める。
* 遠隔のサーバーごとに専用のスレッドを立て、その中で current-thread の tokio runtime を動かす。呼び出し側はコマンドを channel で渡し、応答は std の mpsc で待つ。中断の確認 (`Wait::Cancellable`) と起動の期限は stdio と同じ仕組みで働き、中断すると `notifications/cancelled` を送って POST を止める。runtime はそのスレッドの中で作って捨てるので、TUI の runtime の中で `AgentService` が捨てられても panic しない。接続は 30 秒で開けばよい。呼び出しは stdio と同じく、サーバーの `tool_timeout_secs` を過ぎるか中断で止める。
* ハンドシェイク、`tools/list` のページ送り、`tools/call`、結果の変換は stdio と共有し、送受信だけを `Transport { Stdio, Http }` で分ける。
* 承認は ADR 29 と同じく既定で尋ねる。理由の文だけを「MCP tools run in the <server> server at <origin>; Socium does not control what it does.」にする。stdio の文は変えない。
* 今回も扱わない: HTTP+SSE (2024-11-05)、サーバーからの GET のストリーム、SSE のストリームの再開 (`Last-Event-ID`)。OAuth は ADR 31 で加えた。

### Consequences

* Good, because ホストでプログラムを動かさずに、URL だけで提供されるサーバーの tool を使える。
* Good, because トークンは環境変数から入り、設定ファイルにも画面にも記録にも残らない。
* Good, because 鍵を送ってよい URL の規則がプロバイダーと一つになる。
* Bad, because 遠隔のサーバーは、モデルが渡す tool の引数 (ファイルの中身を含みうる) と設定したヘッダーを受け取る。何をするかはサーバーの運営者次第で、プロバイダーと同じく信頼して登録するしかない。
* Bad, because OAuth しか受け付けないサーバーには接続できない (ADR 31 で `socium mcp login` を加えた)。
* Bad, because セッションが切れると、その時の呼び出しは実行されず、モデルが呼び直す必要がある。新しいセッションで tool の一覧が変わっても、設定を読み直すか再起動するまで反映しない。
* Neutral, because 遠隔のサーバー一つにつきスレッドが一つ増える。

### Confirmation

`settings.rs` のテストで、`command` と `url` の排他、`env` と `headers` の取り違え、https の規則、ヘッダー名の検査と予約されたヘッダー、`${VAR}` の展開と未設定の変数、不正な値で値を示さないこと、未知のキー、`summary()` と `Debug` に値が出ないことを確かめる。`mcp.rs` のテストで、Python の偽のサーバーの HTTP 版 (`crates/core/tests/fixtures/fake_mcp_server.py --http`) を 127.0.0.1 で起動し、JSON と SSE の両方で tool の一覧と結果の変換、SSE の中の ping に答えること、セッション ID とサーバーが選んだ版を以後の要求に付けること、401 と到達できないサーバーの警告、正しいトークンで呼べること、中断がすぐ戻りサーバーに伝わること、404 の後に新しいセッションを始め、呼び出しを送り直さないこと、前のセッションへの遅れた 404 が新しいセッションを消さないこと、通知の 404 で終わったセッションを次の呼び出しの前に始め直すこと、サーバーが書き戻したトークンを伏せ、引用の上限の切れ目にも残さないこと、media type の完全な一致と通知の 202、CR だけの行末と BOM の SSE、チャンクの切れ目によらない SSE の解析と上限、上限を超える行と同じチャンクにある応答を受け取ること、loopback だけがプロキシを使わないこと、registry を捨てると DELETE が届くこと、警告とエラーにトークンが出ないことを確かめる。`lib.rs` のテストで、モデルが遠隔のサーバーの tool を呼び、その結果が次の要求に入り、トークンがモデルへの要求にも進捗のイベントにも出ないことを確かめる。

## Pros and Cons of the Options

### Streamable HTTP だけを、設定したヘッダーで使う

* Good, because 仕様の現行の接続方式だけを実装すればよい。
* Good, because ヘッダーの値を環境から取る仕組みは `env` と同じものを使える。
* Bad, because 古いサーバーと OAuth だけのサーバーには届かない。

### Streamable HTTP に加えて、2024-11-05 の HTTP+SSE にも落ちる

* Good, because 古いサーバーにも接続できる。
* Bad, because GET のストリームと別の POST 先を扱う、もう一つの接続方式を持つことになる。仕様では置き換えられたもので、主要なサーバーには要らない。

### OAuth まで実装する

* Good, because ブラウザで認可するサーバーに、トークンを用意せずに接続できる。
* Bad, because 認可の流れ、トークンの保存と更新、動的なクライアント登録が要り、今回の範囲を大きく超える。

## More Information

[ADR 29](0029-mcpサーバーを利用者の設定だけからstdioで起動しtoolを既定で尋ねて呼ぶ.md) を改める。接続方式は仕様の [Transports](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports)、認可は [Authorization](https://modelcontextprotocol.io/specification/2025-06-18/basic/authorization)。Claude Code の遠隔サーバーは [Claude Code の文書](https://code.claude.com/docs/en/mcp)、OpenCode は [OpenCode の文書](https://opencode.ai/docs/mcp-servers/)。
