---
number: 27
title: 起動中のChromeにCDPで接続しoriginごとの承認でブラウザを操作する
status: proposed
date: 2026-11-20
---

# 起動中のChromeにCDPで接続しoriginごとの承認でブラウザを操作する

## Context and Problem Statement

Socium のモデルは web のページを開けない。作った画面の確認、ログインが要るページの参照、コンソールのエラーの確認を、利用者が代わりに行っている。

エージェントの製品には、専用のプロファイルでブラウザを起動するものがある。そのブラウザには利用者のログインも拡張もなく、利用者はログインをやり直すことになる。操作のたびにスクリーンショットをモデルへ送る方式は、1 手ごとに画像の往復が入る。

一方、ブラウザはサンドボックスの外にある。Socium のコマンドはネットワークに出られないが、ブラウザは出られ、利用者がログインしたサイトでは利用者として振る舞える。ページの内容は信頼できない入力で、モデルへの指示が書かれていることがある ([docs/threat-model.md](../threat-model.md))。

利用者がふだん使うブラウザをモデルが操作する方法と、その範囲を決める。

## Decision Drivers

* 利用者がふだん使うプロファイル (ログイン、拡張) をそのまま使える。
* 1 手ごとの往復と、モデルへ送る量が少ない。
* ワークスペースの内容を、ブラウザ経由で外へ送らせない。
* モデルが Socium 自身の承認の画面を操作できない。
* 有効にする口はモデルから書けない設定に限る ([ADR 12](0012-サンドボックスを広げる口をモデルから書けない設定ファイルに限る.md))。

## Considered Options

* 起動中の Chrome に、Chrome DevTools Protocol (CDP) で接続する
* 拡張と native messaging で接続する
* Socium が専用のプロファイルで Chrome を起動する
* 外部の MCP サーバー (chrome-devtools-mcp) に任せる

## Decision Outcome

Chosen option: 「起動中の Chrome に CDP で接続する」を主とし、「専用のプロファイルで起動する」を隔離のモードとして併せて持つ。利用者のプロファイルに届く方法は CDP での接続と拡張の 2 つで、CDP での接続は配布物を増やさずに済むため。

所有者は 2026-11-20 に、CDP での接続を主とすることを承認した。

#### 接続

* `config.toml` の `[browser]` で有効にする。既定は無効である。
* 接続のモードは 2 つある。
  * `attached`: 利用者が起動している Chrome (144 以降) に接続する。利用者は `chrome://inspect/#remote-debugging` で接続の受け付けを有効にする。Chrome は接続のたびに許可を尋ね、操作されている間はバナーを表示する。
  * `isolated`: Socium が専用のプロファイルで Chrome を起動し、パイプで接続する。待ち受けるポートは開かない。起動するときの環境変数は、Socium が作り直す。
* 接続はセッションの間保つ。接続と操作は Socium の親プロセスが行い、サンドボックスの子プロセスには渡さない。

#### 観測と操作

* ページの観測はアクセシビリティのツリーを文字にして返す。各要素に参照の名前を付け、操作はその名前で指定する。スクリーンショットは、モデルが求めたときだけ返す ([ADR 26](0026-tool結果に画像を持たせファイルに保存して参照で渡す.md))。
* 操作の結果には操作の後のページの観測を含める。
* 操作は手順の並びとして受ける。途中で失敗したら残りを実行せず、どこまで実行したかを返す。
* 待機はページの読み込みと通信の静止の通知で終える。
* tool は、ページを開く、観測する、操作する、本文を読む、スクリーンショットを撮る、コンソールと失敗した通信を読む、タブを一覧する、の 7 つとする。cookie とストレージを返す tool は作らない。任意の JavaScript を実行する tool は、この ADR では入れない。

#### 範囲と承認

* モデルが触れられるのは、Socium が開いたタブだけである。利用者が開いていたタブに触れるには、題名と URL を見せて尋ねる。
* 承認は origin ごとに行う。許可していない origin へ移るときは、URL の全体を見せて尋ねる。リンクやリダイレクトで移る場合も、通信を止めて尋ねる。
* 許可は「読む」と「操作する」の 2 段とする。選択肢に「このセッション中はこの origin を許可」を置く。`browser.allow_origins` に挙げた origin は尋ねない。
* Socium のゲートウェイの origin と、モバイル向けに公開した origin へは、承認の有無にかかわらず移らせない。そこには承認のボタンがある。
* ダウンロードは受け付けない。ファイルの送信はワークスペースの中のファイルに限り、常に尋ねる。
* ページから得た文字には信頼できない内容であることを示す注記を付けてモデルへ返す。
* 答える人がいない実行では、`browser.allow_origins` の origin だけを使える。
* 組み込みのサブエージェントにはブラウザの tool を渡さない。利用者が定めた種類は、tool の一覧に挙げれば使える。

### Consequences

* Good, because 利用者のログインと拡張をそのまま使え、ログインのやり直しが要らない。
* Good, because 観測が文字なので、画像の往復なしに操作を続けられる。
* Good, because 許可していない origin へ URL に載せて送る経路は、承認で人の目を通る。
* Bad, because ブラウザを有効にすると、モデルはブラウザを通じてネットワークに出られる。「コマンドはネットワークに出られない」という保証は、ブラウザには及ばない。脅威モデルに加える。
* Bad, because `attached` では Chrome のバナーと接続のたびの確認が出る。
* Bad, because 許可した origin の中では、モデルは利用者として操作できる。送信や購入のような操作を、origin の許可より細かくは区別しない。
* Bad, because 対応するのは Chrome だけである。Firefox は扱わない。

### Confirmation

実装の前に、`attached` の接続先を見つける方法を実機で確かめる。Chrome の文書には記載がない。CDP のやり取りは記録した通信を返すテスト用のサーバーで確かめる。承認のテストで、許可していない origin への移動が尋ねること、リダイレクトで移る場合も止まること、ゲートウェイの origin を常に断ること、セッション中の許可が 1 つの origin だけを覆うことを確かめる。`isolated` のモードで実際の Chrome を起動し、ローカルのページを開いて、観測、操作、コンソールの取得ができることを確かめる。

## Pros and Cons of the Options

### 起動中の Chrome に CDP で接続する

* Good, because 利用者のプロファイルをそのまま使える。配布物は Socium だけで済む。
* Bad, because Chrome 144 以降に限る。バナーと確認は消せない。

### 拡張と native messaging で接続する

* Good, because バナーが出ず、サイトごとの権限を拡張の仕組みで持てる。Firefox にも広げられる。
* Bad, because 拡張の配布、審査、更新を続ける必要がある。

### Socium が専用のプロファイルで Chrome を起動する

* Good, because 利用者のログインに触れないので、信頼できないページとローカルの確認に向く。
* Bad, because 利用者のログインと拡張を使えない。この方法だけではふだん使うプロファイルを使うという要求を満たさない。

### 外部の MCP サーバーに任せる

* Good, because Socium に実装が要らない。MCP の接続が入れば、設定だけで使える。
* Bad, because 承認が tool の単位になり、origin ごとには尋ねられない。サーバーを起動する Node も要る。

## More Information

Chrome 136 以降、既定のプロファイルではデバッグ用の起動引数が無視される ([Changes to remote debugging switches](https://developer.chrome.com/blog/remote-debugging-port))。起動中の Chrome への接続は Chrome 144 以降で使える ([Auto-connect](https://developer.chrome.com/docs/devtools/agents/use-cases/auto-connect.md.txt)、[Chrome DevTools MCP の記事](https://developer.chrome.com/blog/chrome-devtools-mcp-debug-your-browser-session))。web とモバイルのクライアントは [docs/clients.md](../clients.md)。
