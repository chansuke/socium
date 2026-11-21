---
number: 31
title: 遠隔のMCPサーバーをCLIのOAuthログインで認可しトークンを利用者の状態に保つ
status: proposed
date: 2026-11-19
links:
- target: 30
  kind: amends
---

# 遠隔のMCPサーバーをCLIのOAuthログインで認可しトークンを利用者の状態に保つ

## Context and Problem Statement

ADR 30 は遠隔の MCP サーバーへの認証を、設定したヘッダーに限った。Linear (`https://mcp.linear.app/mcp`)、Notion、Sentry のように、OAuth だけを受け付け、利用者に渡す固定のトークンを持たないサーバーがある。こうしたサーバーは、MCP 2025-06-18 の [Authorization](https://modelcontextprotocol.io/specification/2025-06-18/basic/authorization) に従い、401 と保護されたリソースのメタデータ (RFC 9728) で認可サーバーを示し、動的なクライアント登録 (RFC 7591) を受け付ける。Claude Code と OpenCode は、ブラウザでの認可でこれらに接続できる。socium で、誰がいつ認可の手続きを始め、トークンをどこに置き、実行中にどう使い更新するかを決める。

## Decision Drivers

* ADR 30 と同じく、トークン、認可コード、クライアントの秘密、PKCE の verifier は、警告、エラー、`Debug`、activity、ログ、system prompt、モデルへの要求のどれにも出さない。
* トークンはワークスペースに置かない。モデルのファイル tool とコマンドはワークスペースを読めるので、そこに置くとモデルに渡りうる。
* 認可には人がブラウザで同意する必要がある。`socium -p` や、起動中のサーバーの接続の途中でブラウザを待つわけにはいかない。
* 認可サーバーへの要求は、プロバイダーと同じ規則 (https、平文の http は loopback だけ、redirect をたどらない) に従う。
* 新しい crate を増やさない。SHA-256 と乱数は rustls がすでに使う aws-lc-rs、base64 もすでに依存の木にあるものを使う。

## Considered Options

* CLI のサブコマンド `socium mcp login` で認可し、トークンを利用者ごとの状態ディレクトリに保つ
* TUI の中で、サーバーが 401 を返したときに認可を始める
* OAuth を実装せず、利用者が別の手段で得たトークンをヘッダーに書く (ADR 30 のまま)

## Decision Outcome

Chosen option: 「CLI のサブコマンド `socium mcp login` で認可し、トークンを利用者ごとの状態ディレクトリに保つ」。ブラウザを待つ処理を通常の起動から切り離せ、TUI と `socium -p` の両方が同じ保存済みのトークンを使える。

* `socium mcp login <server>` は、`config.toml` の `url` を持つサーバーに対して次を行う。サーバーに認証なしで initialize を送り、401 の `WWW-Authenticate` の Bearer の challenge の `resource_metadata` から、無ければ RFC 9728 の well-known の URL (パスを挟んだもの、次にルート) から保護されたリソースのメタデータを読む。`WWW-Authenticate` は一つの値に複数の challenge を持ちうるので、引用符の中のカンマを区切りと見ない challenge の列として読む。challenge は token68 か parameter のどちらかを持つ (RFC 9110 11.6.1) ので、`Basic dXNlcjpwYXNz=` は token68 と読み、token68 の後の parameter はどの challenge のものともしない。その `authorization_servers` の先頭を認可サーバーとし (`oauth.issuer` があれば、その値と一字一句同じものを選ぶ)、RFC 8414 の URL、次に OpenID Connect Discovery の二つの URL からメタデータを読む。メタデータの `issuer` が認可サーバーの URL と一字一句同じでなければ誤りにする。
* サーバーやメタデータが示した URL (`resource_metadata`、認可サーバー、各エンドポイント) は、設定の `url` 自身がそうでない限り、このマシンや私的なネットワークを指してはならない (`localhost` と、loopback、private、link-local などのアドレス)。遠隔のサーバーが、ログインにマシンの内側のサービスを叩かせることを防ぐ (RFC 9728 7.7)。名前は一度だけ解決し、答えに内側のアドレスが一つでもあれば拒む。要求はその答えのアドレスに固定し (`resolve_to_addrs`)、環境のプロキシを通さずに直接送る。二度目の解決で内側に切り替える DNS rebinding も、名前を自分で解決するプロキシも通らない。検査しないのは、設定の `url` と、そこから導いた RFC 9728 の well-known の URL だけで、同じホストとポートでもほかのパスは検査する。認可の URL も、ブラウザに渡して表示する前に同じく解決して検査する。ただしブラウザは名前を自分で解決し直すので、ブラウザの側の DNS rebinding は socium には防げない。
* 設定の `oauth.client_id` が無ければ、`registration_endpoint` に公開クライアント (`token_endpoint_auth_method = "none"`) として登録する。登録の口が無ければ、`oauth.client_id` を設定するよう伝えて失敗する。
* 127.0.0.1 で待ち受け (`oauth.callback_port` が無ければ空いているポート)、redirect URI を `http://127.0.0.1:<port>/callback` にする。PKCE は S256 だけを使い、認可サーバーが `code_challenge_methods_supported` に S256 を含めなければ、その欄が無いときも含めて誤りにする。ランダムな `state` を付け、戻ってきた値が違えばその答えを捨ててログインを失敗させる。scope は `oauth.scopes`、無ければメタデータの `scopes_supported`、どちらも無ければ送らない。
* `resource` (RFC 8707) は認可要求とトークン要求の両方に付ける。値は保護されたリソースのメタデータの `resource` で、それがサーバーの URL と同じ origin でパスが URL を覆い、fragment を持たないときに限り受け入れる。覆わなければ誤りにし、`resource` が無いメタデータも RFC 9728 3.2 に従って誤りにする。RFC 9728 3.3 は完全な一致を求めるが、`/mcp` のサーバーが origin を `resource` に書く例があるので、上位のパスは受け入れる。
* 認可の URL を標準エラーに出し、`xdg-open` (macOS では `open`) を試す。無くても失敗しない。opener は、相対の項目とワークスペースの中の項目を除いた PATH から探し、見つけたファイルの実体 (シンボリックリンクの先) がワークスペースの中にあれば使わない。`/` で、デスクトップに要る変数 (HOME、DISPLAY、WAYLAND_DISPLAY、DBUS_SESSION_BUS_ADDRESS など) だけを渡して起動する。プロバイダーの API キーと、xdg-open が既定のブラウザの代わりに動かす `BROWSER` は渡さない。
* `socium mcp login` と `logout` も、通常の起動と同じく、設定とトークンの置き場がワークスペースの中にあれば拒む。トークンの置き場は絶対パスの `XDG_STATE_HOME` か `HOME` からだけ決める。ブラウザの戻りを 5 分待ち、短いページで答え、コードをトークンに交換して保存する。失敗すれば理由を一文で示して 0 以外で終わる。
* `oauth` の表 (`client_id`、`client_secret`、`issuer`、`token_endpoint_auth_method`、`scopes`、`callback_port`) は `url` のサーバーにだけ書ける (`command` では誤り)。`client_secret` は `client_id` と `issuer` と組で、`${NAME}` をヘッダーと同じく展開する。`issuer` を必須にするのは、侵害された MCP サーバーが別の認可サーバーを示しても、秘密をそこに送らないため。`token_endpoint_auth_method` (`client_secret_basic` か `client_secret_post`) はそのクライアントの登録の方法で、無ければ認可サーバーが示す対応から選ぶ。動的な登録では、登録の応答が示す方法をそのまま使い (無ければ RFC 7591 の既定の `client_secret_basic`)、対応しない方法、また `client_secret_basic` か `client_secret_post` なのに `client_secret` が無い応答は誤りにする。保存したトークンのクライアントが設定の `client_id` と違えば、そのトークンも設定の秘密も使わない。設定に `issuer` があれば、保存したトークンの issuer がそれと一字一句同じでなければ (issuer を持たない以前のトークンも) 同じく使わず、ログインを求める。同じ `client_id` の別の認可サーバーに切り替えたとき、新しい秘密が前のトークンエンドポイントに送られないため。この表は事前に登録したクライアントのためのもので、無くても OAuth は使える。
* トークンは `$XDG_STATE_HOME/socium/mcp-auth/<server>.json` (無ければ `~/.local/state/socium/mcp-auth/<server>.json`) に置く。ディレクトリは 0700、ファイルは 0600 で、セッションの状態と同じ原子的な書き込みで置き換える。シンボリックリンク、他人の所有物、所有者のほかが入れるディレクトリは、読むときも書くときも消すときも拒む (ディレクトリがまだ無ければ、読むときはトークンが無いだけとする)。中身はアクセストークン、リフレッシュトークン、期限と発行の時刻、トークンエンドポイントと、メタデータで確かめた issuer、`resource`、登録で得たクライアントの ID と秘密、ログインしたときの URL である。設定の `client_secret` は書かない。読み直せない大きさ (256 KiB 超) のトークンは保存しない。保存した URL が設定の URL と違えば、そのトークンは使わない。ワークスペースがこのディレクトリを含むと、設定のディレクトリと同じく起動を拒む。
* `socium mcp logout <server>` はそのファイルを消す。設定にサーバーが無くても消せる。トークンの失効 (RFC 7009) は送らない。
* 実行中は、保存したトークンがあるサーバーへのすべての要求に `Authorization: Bearer` を付ける (値は sensitive)。期限の 60 秒前 (寿命の半分がそれより短ければ寿命の半分) を過ぎていればリフレッシュトークンで更新し、新しいトークンを保存する。401 が返れば一度だけ更新して送り直す。更新に失敗したとき、トークンが無く 401 が返ったときは、そのサーバーを起動せず「MCP server <name> needs you to log in: run socium mcp login <name>」と警告する。呼び出しの途中なら同じ文を tool のエラーにする。
* 更新は、同じ利用者のすべての socium が `<server>.lock` の flock を取ってから行い、取った後にファイルを読み直す。`socium mcp login` も同じロックを取ってから保存する。ファイルが無ければ利用者がログアウトしたので、更新もせず書き戻しもしない。ファイルの記録全体が、このセッションが最後に読んだか保存した記録と違えば、別の socium が更新したか利用者がログインし直したので、その記録を使う。ログアウトも同じロックを取ってから消すので、更新中の socium がファイルを復活させず、更新中にログインし直しても、古いセッションの更新がその新しいトークンを上書きしない。ロックは閉じる前に `LOCK_UN` で外す。fork した子が exec するまで記述子の写しを持つ間も、ロックが残らないため。更新は呼び出しとは別の task で走らせ、呼び出しが中断されても、認可サーバーが発行したトークンを最後まで受け取って保存する。サーバーを捨てるときも、進行中の更新をトークン要求の期限 (30 秒) まで待つ。保存に失敗したトークンは、それが置き換えた記録と一緒にメモリに残して使い続け、次の turn の始めに警告を出し、次の要求の前と終了時に保存し直す。ファイルがその記録のままなら、メモリのトークンが新しいので、次の更新もメモリのリフレッシュトークンから行う。ファイルが別の記録になっていればそれを使い、無くなっていれば書き戻さず、読めなければ後でまた試す。
* 設定の `headers` に `Authorization` があれば OAuth より優先し、保存したトークンは読まない。そのヘッダーへの 401 は、ヘッダーが拒まれたと伝える。
* 認可サーバーへの要求は MCP の接続と同じ reqwest の client (redirect をたどらない) で送り、30 秒で諦める。メタデータが示す各エンドポイントは `models::check_endpoint_url` で確かめる。エラーに引用するのは OAuth の `error` の符号だけで、それも RFC 6749、7591、6750、8707 と OpenID Connect が定める符号に限る。`error_description`、知らない符号、Bearer でない `token_type` は引用しない (送ったコードや秘密を繰り返すサーバーがありうるため)。MCP サーバーが、HTTP のエラー、JSON-RPC のエラー (JSON の本文でも SSE でも)、tool のエラーの結果、起動の失敗に、送られた Bearer のトークンを書き戻したときは、このセッションが送ったアクセストークンを、引用を切り詰める前に `[redacted]` に置き換える (ADR 30 のヘッダーの値と同じく 8 文字以上のもの)。プロキシは、設定の `url` とその well-known の URL への要求では環境の設定に従い (loopback を除く)、発見した URL への要求では使わない。

### Consequences

* Good, because OAuth だけを受け付けるサーバーに、利用者がトークンを用意せずに接続できる。
* Good, because トークンはワークスペースの外の、利用者だけが読めるファイルにあり、モデルにも記録にも出ない。
* Good, because TUI と `socium -p` が同じトークンを使い、期限が来ても自動で更新される。
* Bad, because ログインのたびに動的な登録で新しいクライアントを作る (redirect URI のポートが毎回変わるため)。認可サーバーに使わないクライアントが残る。
* Bad, because トークンは暗号化せずにファイルに置く。同じ利用者として動くプロセスは読める。OS の keyring は使わない。
* Bad, because トークンを持つサーバーは、利用者がブラウザで同意した範囲で、利用者として振る舞える。何を許したかは認可サーバーの同意画面でしか分からない。
* Bad, because 発見した認可サーバーへの要求は環境のプロキシを通らないので、直接の外向きの通信を許さないネットワークでは、別のホストにある認可サーバーにログインも更新もできない。
* Bad, because ブラウザは認可の URL の名前を自分で解決し直すので、ブラウザの側の DNS rebinding は防げない。
* Neutral, because ログインの確認に人のブラウザ操作が要るので、実在のサーバーに対する自動の確認は、発見までに限られる。

### Confirmation

`settings.rs` のテストで、`oauth` の値と `Debug`・`summary()` に秘密が出ないこと、未設定の `${NAME}` の警告、`command` との組み合わせ、`client_id` や `issuer` の無い `client_secret`、`token_endpoint_auth_method` の誤り、scope と port の誤りを確かめる。`token_store.rs` のテストで、0700 と 0600、URL の違うトークンを無視すること、ファイル、ロック、ディレクトリのシンボリックリンク (先の無いものも) と、所有者のほかが入れるディレクトリを、読むときも書くときも消すときも拒むこと、fork した子がロックを残さないこと、issuer を持たない以前のファイルを読めること、壊れたファイルのエラーに中身が出ないこと、相対の HOME を使わないこと、大きすぎるトークンを保存しないこと、寿命の短いトークンの更新の時期、ロックが一つの持ち主にしか渡らないことを確かめる。`oauth.rs` のテストで、偽のサーバー (`fake_mcp_server.py --http --oauth`) を認可サーバーとして使い、ブラウザの代わりにテストが認可の URL とコールバックを GET して、401 のヘッダーと well-known と OpenID Connect からの発見、動的な登録、PKCE (偽のサーバーが verifier を確かめる) と RFC 7636 の例の値、`state` の不一致と認可サーバーの拒否で何も保存しないこと、`resource` が両方の要求に付くこと、トークンが次の接続で再利用されること、期限前の更新と 401 の後の更新、更新の失敗と別の URL のトークンでのログインの警告、`Authorization` ヘッダーの優先、ログアウト、どの出力にもコード・verifier・トークンが出ないことを確かめる。さらに、`resource` や S256 の無いメタデータと一覧に無い `issuer` を拒むこと、内側のアドレスの判定、内側に解決される名前を拒むことと、二度目の解決が内側を返しても要求が届かないこと、一つの値に複数の challenge を持つ `WWW-Authenticate`、登録の応答が示す認証の方法、知らない OAuth の符号を引用しないこと、ログアウト後の更新がファイルを書かないこと、別の socium が更新したトークンを使うこと、待っていた呼び出しが中断されても、サーバーを捨てても更新が保存されること、保存の失敗が続いてもメモリの新しいトークンから更新し、保存できたときに書くこと、ほかがロックを持つ間はメモリのトークンを使い、外れた後に保存すること、更新の途中のログインが上書きされないこと、設定と違うクライアントや issuer のトークンを使わないこと、token68 を含む `WWW-Authenticate`、secret も方法も無い登録の応答を拒むこと、内側に解決される認可の URL をブラウザに渡さないこと、well-known でない同じホストの URL を検査すること、発見した URL がプロキシを通らないこと、Bearer のトークンを HTTP と JSON-RPC と SSE のエラー、tool のエラーの結果、起動の警告から引用の上限の切れ目も含めて伏せること、ワークスペースが設定やトークンの置き場を含むとログインを拒むことを確かめる。TUI のテストで、opener を探す PATH からワークスペースを除くこと、ワークスペースに通じるシンボリックリンクの opener を使わないこと、`BROWSER` とプロバイダーの API キーを渡さないことを確かめる。`lib.rs` のテストで、ログイン後にモデルがそのサーバーの tool を呼べて、トークンがモデルへの要求にも進捗のイベントにも出ないことを確かめる。

## Pros and Cons of the Options

### CLI のサブコマンド `socium mcp login` で認可し、トークンを利用者ごとの状態ディレクトリに保つ

* Good, because ブラウザを待つのはログインのときだけで、起動と `socium -p` は待たない。
* Good, because 失敗を終了状態と一文で示せる。
* Bad, because 利用者は、起動時の警告を見てから別の端末でコマンドを打つ必要がある。

### TUI の中で、サーバーが 401 を返したときに認可を始める

* Good, because 利用者は TUI から離れずに済む。
* Bad, because 起動中の接続がブラウザを待つことになり、`socium -p` では使えない。TUI の描画とブラウザの URL の表示を両立させる仕組みも要る。

### OAuth を実装せず、利用者が別の手段で得たトークンをヘッダーに書く (ADR 30 のまま)

* Good, because 実装を増やさない。
* Bad, because OAuth だけのサーバーには接続できず、利用者が自分でトークンを取り、期限が来るたびに入れ替える必要がある。

## More Information

[ADR 30](0030-遠隔のmcpサーバーにstreamable-httpとヘッダーだけで接続する.md) を改める。根拠は MCP の [Authorization](https://modelcontextprotocol.io/specification/2025-06-18/basic/authorization)、[RFC 9728](https://datatracker.ietf.org/doc/html/rfc9728)、[RFC 8414](https://datatracker.ietf.org/doc/html/rfc8414)、[RFC 7591](https://datatracker.ietf.org/doc/html/rfc7591)、[RFC 7636](https://datatracker.ietf.org/doc/html/rfc7636)、[RFC 8707](https://www.rfc-editor.org/rfc/rfc8707.html)、[RFC 8252](https://datatracker.ietf.org/doc/html/rfc8252) (ネイティブアプリの loopback の redirect)。Linear、Notion、Sentry、GitHub の MCP サーバーに対して、発見までが働くことを手で確かめた (GitHub は動的な登録を持たないので `oauth.client_id` が要る)。
