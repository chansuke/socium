---
number: 24
title: MCPサーバーを利用者が選べば読み取り専用のサンドボックスで動かしtoolを尋ねずに呼ぶ
status: proposed
date: 2026-11-19
links:
  - target: 20
    kind: amends
  - target: 23
    kind: amends
---

# MCPサーバーを利用者が選べば読み取り専用のサンドボックスで動かしtoolを尋ねずに呼ぶ

## Context and Problem Statement

ADR 20 は MCP サーバーをホストで動かし、tool を既定で尋ねて呼ぶ。多くのサーバーはネットワークとホストの認証情報を必要とし、それはサンドボックスの中では用意できないからである。ADR 20 は「サーバーをサンドボックスの中で動かす」案を、多くのサーバーが動かないという理由で退けた。

一方、ネットワークも認証情報も要らないサーバーもある。ワークスペースを読んで答えるだけの、ローカルの解析系である。こうしたサーバーにまでホストの権限を与え、呼ぶたびに尋ねる理由はない。

具体例として、構文木で探す検索を考える。`search_text` は文字列の正規表現で、`find_symbol` は Rust の宣言らしい行を探す heuristic にすぎない (SOC-025)。「`.unwrap()` の呼び出しをすべて」のような問いには、ast-grep (https://ast-grep.github.io/ ) が向いている。

いまの経路で ast-grep を承認なしに使うと、どれも書き換えまで許すことになる。

* `run_command` を `approval.allow = ["ast-grep run"]` や `approval.commands = "auto"` で許すと、`ast-grep run -p X -r Y -U` による workspace の一括書き換えまで承認なしで通る。承認なしで動くコマンドも workspace に書けるからである (`WorkspaceAccess::Write`)。
* ast-grep を包んだ MCP サーバーを ADR 20 の `allow` で許すと、サーバーはサンドボックスの外で、ホストの権限のまま動く。

この ADR は、OpenCode や Claude Code のようなプラグインの仕組みを socium に入れるかの検討から生まれた。検討の結果、ローカルの解析系を足すための第三者のコードの入れ物としては、ADR 20 の MCP サーバーで足りると判断した。足りないのは、そのサーバーを閉じ込めて動かす方法だけである。

ここで決めるのは三つである。利用者が選んだ MCP サーバーを、読み取り専用のサンドボックスで動かせるようにするか。そのとき何を見せ、承認をどう変えるか。サブエージェントへの渡し方をどう変えるか。

## Decision Drivers

* サンドボックスで動かすかどうかを決めるのは、利用者の `config.toml` だけにする (ADR 12、ADR 20)。サーバーの申告やモデルの出力では決まらない。
* サンドボックスで動かすサーバーは、workspace に書けず、ネットワークに出られず、ホストの HOME と認証情報が見えない。これらを socium の既存の executor (bubblewrap と seccomp) が強制する。
* 外部の tool を呼ぶ経路は、ADR 20 の MCP クライアントの一つのままにする。プラグインのための別の経路を作らない。
* ADR 20、21、23 の、ホストで動くサーバーの挙動を変えない。
* 一例のために仕組みを大きくしない。組み込みの tool と同じ条件で比べてから採る。

## Considered Options

* 今のまま。ast-grep は `run_command` か、ホストで動く MCP サーバーで使う。
* ast-grep を、設定で有効にする組み込みの tool にする (`web_search` と同じ型)。
* `[mcp.servers.<name>]` に `sandbox = "read-only"` を足し、選んだサーバーを読み取り専用のサンドボックスで動かして、tool を尋ねずに呼ぶ。
* 設定ディレクトリに置くプラグインの仕組みを別に作る。manifest、配置の検査、呼び出しごとの起動を持つ。
* hook を中心にしたプラグイン (Claude Code の hooks の型)。
* 同じプロセスで動くプラグイン (OpenCode の型。dylib、組み込みの JS、WASM)。

## Decision Outcome

Chosen option (proposed): 「`sandbox = "read-only"` を足す」。ADR 20 のクライアント、プロトコル、registry、サーバーの寿命はそのまま使い、変えるのはサーバーの起動のしかた、承認、サブエージェントでの分類だけである。

この ADR を accepted にするのは、Confirmation の比較で採用の条件を満たしたときだけである。

### 設定

```toml
[mcp.servers.astgrep]
command = ["astgrep-mcp"]
sandbox = "read-only"
```

* `sandbox` は省略するか、`"read-only"` だけを書ける。省略すれば ADR 20 のとおりホストで動く。
* `sandbox` は `command` のサーバーにだけ書ける。`url` (ADR 21) と組み合わせれば誤りとする。
* `sandbox` と `allow` は一緒に書けない。サンドボックスのサーバーの tool はすべて尋ねずに動くので、`allow` に意味がないからである。
* `read_only` (任意) には、サーバーのためにだけ読み取り専用で見せるホストのディレクトリを書く。
  * 検査は ADR 12 の `sandbox.read_only` と同じである。ホームとその祖先、`/`、予約されたパスは誤りとする。
  * 加えて、ワークスペース、`.socium`、socium の設定ディレクトリと重なるものも誤りとする。モデルが書ける場所や設定をサーバーに読ませないためである。
* 知らない項目は、ADR 20 と同じく誤りにする。

### 起動

* サーバーは `SandboxExecutor` で起動する。方針 (`ExecutionPolicy`) は次のとおり。
  * workspace は `WorkspaceAccess::ReadOnly` でマウントする。ネットワークは閉じ、HOME と tmp は private にする。cwd は workspace のルートとする。ADR 20 はホストのサーバーの cwd を `/` にするが、それはワークスペースのモジュールがホストで動くのを防ぐためである。ここでは読み込まれても動くのはサンドボックスの中で、ast-grep の例のように workspace からの相対パスで探す道具が使える。
  * `sandbox.read_only`、`sandbox.path`、`sandbox.env` は継承しない。これらはビルドとテストのために利用者が開いたもので、サーバーに渡す理由がない。見えるのは、runtime パス (`/nix/store`、`/usr` など) と、このサーバーの `read_only` だけである。
  * PATH は runtime パスだけにする。ワークスペースの中を指す項目はもともと入らない。`command` の先頭は、サンドボックスの中で見える場所に解決されなければならない。runtime パスの中か、このサーバーの `read_only` の中である。Nix で入れたサーバーなら、そのまま名前で書ける。`./server` のような相対パスは ADR 20 と同じく設定を読む時点で拒否する。
* 環境変数。
  * ADR 20 が渡す PATH、HOME、USER、LANG は渡さない。HOME と PATH はサンドボックスが決める。
  * `env` は、サンドボックスができた後の子にだけ、bubblewrap の `--setenv` で渡す。`--setenv` の並びは `sandbox.env` と同じく記述子 (`--args`) から bubblewrap に読ませ、bubblewrap 自身の環境にもコマンドラインにも入れない。`LD_PRELOAD` のようなローダーの変数が、サンドボックスの外で bubblewrap に読み込まれないようにするためと、誰でも読める `/proc/<pid>/cmdline` に値を出さないためである。
  * 名前の検査は ADR 12 の `sandbox.env` と同じとする。
* 起動の時点で `approval::restrictions` の制限を計算し、`approval.sensitive` に当たるファイルを空のファイルとして見せる。制限の計算が失敗したら (列挙が時間を超えた、256 件を超えた、など)、そのサーバーを起動しない。起動しない理由は、ADR 20 の他の失敗と同じく警告に出す。承認に切り替えることも、制限なしで動かすこともしない。
* ここから後は、ADR 20 と同じである。
  * ハンドシェイク、`tools/list`、30 秒の起動の期限と、起動の中止。
  * `tool_timeout_secs` による呼び出しの期限。
  * tool の数と定義の大きさの上限。サンドボックスのサーバーも、ホストのサーバーと同じ全体の上限に数える。
  * stdin への書き込みを専用の thread で行うこと。
  * `notifications/cancelled` による中断。
  * registry がサーバーを持ち、捨てれば process group ごと終わらせること。
* registry が持つ子プロセスは bubblewrap になる。bubblewrap は `--unshare-all` で PID namespace を分け、`--die-with-parent` を付けている。そのため bubblewrap を終わらせれば、サンドボックスの中のプロセスは子孫まで消える。`setsid` した子孫が消えることは、既存のテストで確かめている (`execution/tests.rs` の setsid のケース)。
* 終了の手順は ADR 20 と同じく、stdin を閉じて 1 秒、SIGTERM を送って 1 秒、SIGKILL、回収の順である。signal を受けるのは bubblewrap のプロセスグループで、SIGTERM がサンドボックスの中のサーバーに届くか、bubblewrap が先に終わって namespace ごと消えるかは未確認である。どちらでも子孫は残らないので、穏やかに終わる機会は stdin を閉じる段階だと考える。実装のテストで確かめる。

### 見える範囲と保証

* 保証するのは、起動の時点で `approval.sensitive` に当たり、空のファイルとして隠せた workspace のファイルの中身に、サーバーが届かないことである。次のものは例外として明示する。
  * `.git` と、linked worktree の共通の Git ディレクトリ。読み取り専用で見える。列挙は `.git` を除くので、中に置かれた認証情報は隠れない。
  * `.socium/scratch`。読み取り専用で見える。
  * サーバーの起動の後に作られた秘密のファイル。サーバーはセッションの間動き続けるので、その間に作られたものは隠れない。
* サーバーから見えるのは、workspace の中で上の保証の対象にならないもの、上の例外、runtime パス、このサーバーの `read_only` である。
* 見える範囲は `read_file` より広く、読んだ内容は tool の結果としてモデルの provider に送られうる。
  * 起動時に、サンドボックスで動くサーバーと、この範囲を警告欄に出す。
  * threat model の表には、実装と同じ変更で行を足す。例外もそのまま書く。

### 承認

* サンドボックスで動くサーバーの tool は、`mcp_gate` で尋ねずに動かす。
* 前提は、上の方針で workspace に書けず、ネットワークがなく、保証の対象の秘密が空に見えることである。このとき最悪の場合でも、見える範囲を読み、結果として provider に送るところまでにとどまる。
* 承認を求めたいときは、`sandbox` を外してホストで動かす (ADR 20 のとおり尋ねる) か、サーバーを `enabled = false` にする。

### サブエージェント (ADR 23 の変更)

* サンドボックスで動くサーバーの tool は、`may_change_workspace` で workspace を変えない tool に数える。
  * 根拠にするのは、設定の `sandbox` だけである。サーバーの申告は根拠にしない。
  * 定義を読む時点でも、設定はもう読まれている。そのため、サーバーの接続を待たずに分類できる。
* したがって、ADR 23 の「MCP の tool を一つでも持つ種類は並行に動かない」は、ホストで動くサーバーの tool にだけ当てはまる。
* `explorer` と、`tools` を省いた種類には、ADR 23 のとおり MCP の tool を渡さない。読み取り専用の種類の既定を、利用者が足したサーバーで広げないためである。

### 例: ast-grep

2026-11-19 に nixpkgs `e21fec321d73511849707a7585e4ea155ee4bf40` で `nix search nixpkgs ast-grep` を実行した。`ast-grep` (0.51.2) はあり、ast-grep の MCP サーバーはなかった。そのため、例のサーバー `astgrep-mcp` は自分で書く。

* 作り方。Python 3 で書いた MCP の stdio サーバーで、Home Manager などで Nix のパッケージにして入れる。こうするとサーバーの本体も ast-grep も `/nix/store` にあり、`read_only` は要らない。
* tool は `search` の一つで、モデルからは `mcp__astgrep__search` に見える。
* 入力の検証。
  * `pattern` (必須) は 4096 文字以内の文字列。
  * `lang` (必須) は、ast-grep の組み込みの言語名の一覧にあるもの。
  * `paths` (任意) は、workspace からの相対パスの文字列を 32 個まで並べた配列。空なら `.` とする。
  * `paths` は、絶対パスと `..` を含むものを誤りにする。そのうえで、解決した先が workspace のルートの中になければ誤りにする。
* 実行する argv は次のとおりで、cwd は workspace のルートである。

  ```text
  ast-grep run --config <package>/share/astgrep-mcp/sgconfig.yml --pattern <pattern> --lang <lang> --json=stream -- <paths>...
  ```

  * `--follow` は渡さないので、ast-grep は探索の途中で symlink を辿らない (`ast-grep run --help` で確認)。
  * `sgconfig.yml` はパッケージに同梱する。中身は、ルールのディレクトリも `customLanguages` も持たない最小の設定である。workspace の `sgconfig.yml` を読まないためで、`ast-grep run` は既定で workspace の設定を読み、そこで宣言された tree-sitter の共有ライブラリを読み込みうる。ast-grep の文書による記述で、手元では未確認である。共有ライブラリが読み込まれたとしても、閉じ込める先はこの ADR のサンドボックスである。
  * `--rewrite` と `--update-all` は渡さない。仮に渡したとしても、workspace が読み取り専用なので書き込みは失敗する。
* 結果の扱い。
  * stream の出力を一行ずつ読み、200 件目を読んだら ast-grep を止める。読む量は合計 2 MiB までとする。
  * 返すのは各件の `file`、`range.start`、`lines`、`metaVariables.single` である。
  * 200 件に達したら、「少なくとも 200 件あり、以降は打ち切った」と添える。
* ast-grep は名前で引く。Ubuntu の `/usr/bin/sg` は login パッケージの `sg` なので、別名の `sg` は使わない。

### Consequences

* Good, because 構文木の検索を、承認なしで、しかも書き換えられないことがサンドボックスで保証された形で使える。`approval.allow`、`auto`、ADR 20 の `allow` では、この保証は得られない。
* Good, because ローカルで完結する解析系なら、socium のリリースなしに足せる。外部の tool を呼ぶ経路は ADR 20 の一つのままである。
* Good, because 足すのは設定の項目二つ (`sandbox`、`read_only`) と、起動、承認、分類の分岐だけである。プラグインのための読み込み、manifest、別のプロトコルは要らない。
* Good, because サンドボックスで動くサーバーの tool は、サブエージェントの中で並行に動ける。
* Bad, because ネットワークや HOME を使うサーバー、`npx` や `uvx` で起動時に取得するサーバーは、サンドボックスでは動かない。これらは ADR 20 のとおりホストで動かす。
* Bad, because 見える範囲は `read_file` より広く、`.git`、`.socium/scratch`、起動の後に作られた秘密のファイルは隠れない。
* Bad, because ast-grep の一例だけなら、組み込みの tool の方が小さい。採否を Confirmation の比較に掛けるのはこのためである。

### Confirmation

採用の条件は次のとおりで、比較の前に固定する。

* 比べる三つ。
  * (a) 現状 (`run_command`)
  * (b) 設定で有効にする組み込みの ast-grep tool
  * (c) この ADR のサンドボックスで動く `astgrep-mcp`。(b) と (c) は同じ入力と出力にする。
* task。SOC-032 の評価に、構文木の検索が要る task を三つ以上足す。成功は、各 task の検査が通ることとする。
* 試行。task ごと、比べる三つのそれぞれについて、同じモデルと同じ予算で 5 回以上走らせる。
* 記録するもの。成功率、承認の回数、所要時間の中央値、tokens、起動時間、一回の検索の遅延。
* (b) か (c) を (a) に対して採る条件は、次の三つをすべて満たすことである。
  * 成功率が (a) 以上である。
  * 承認の回数が (a) より少ない。
  * 所要時間の中央値の増加が 20% 以内である。
* (c) を (b) より先に採る条件は、次の三つをすべて満たすことである。
  * 成功率の差が (b) から 5 ポイント以内の低下にとどまる。
  * 所要時間の中央値の増加が 30% 以内、tokens の増加が 10% 以内、起動時間の増加が 1 秒以内である。
  * 下の必須のテストがすべて通る。
* 判断。
  * (c) が条件を満たせば、この ADR を accepted にする。
  * (c) が満たさず (b) が満たせば、(b) を別の ADR で採り、この ADR は rejected にする。
  * どちらも満たさなければ何も入れず、この ADR は rejected にする。
  * 結果は SOC-036 に記録する。

必須のテスト。

* `settings.rs` のテスト。
  * `sandbox` の値の検査。`url`、`allow` との組み合わせの拒否。
  * `read_only` の検査。ホームとその祖先、予約されたパス、ワークスペースと設定ディレクトリに重なるものの拒否。
* `execution/tests.rs` と `mcp.rs` のテスト。偽のサーバー (`fake_mcp_server.py`) をサンドボックスで起動し、次のことを確かめる。
  * workspace への書き込み、ネットワーク、ホストの HOME、`sandbox.read_only` と `sandbox.env`、保証の対象の秘密のファイルの中身に届かないこと。
  * `env` が子に届き、bubblewrap 自身の環境にもコマンドラインにも入らないこと。
  * 終了の手順で、stdin を閉じても終わらないサーバーが SIGTERM と SIGKILL の段階で消えること。
  * 秘密のファイルの列挙が失敗したら、サーバーを起動しないこと。
  * registry を捨てると、`setsid` した子孫まで消えること。
* `approval.rs` のテスト。サンドボックスのサーバーの tool が尋ねずに動き、ホストのサーバーの tool は ADR 20 のとおり尋ねること。
* `catalog.rs` とサブエージェントのテスト。サンドボックスのサーバーの tool だけを持つ種類が workspace を変えない種類に数えられること、`explorer` には渡らないこと。
* ast-grep の例について。悪意のある `sgconfig.yml` を置いた workspace でその設定が読まれないこと、workspace の外を指す symlink の `paths` が拒否されることを確かめる。

## Pros and Cons of the Options

### 今のまま

* Good, because コードが増えない。
* Bad, because 毎回またはセッションごとに承認するか、書き換えやホストでの実行まで承認なしで許すかのどちらかになる。

### 設定で有効にする組み込みの ast-grep tool

* Good, because サンドボックスの起動や MCP の往復が要らない。この一例だけなら最も小さい。
* Good, because `web_search` と同じく設定で有効にする形にすれば、使わない利用者に依存も起動時の警告も増えない。
* Bad, because 解析系を一つ足すたびに socium の変更とリリースが要る。

### `sandbox = "read-only"` を足す (この ADR)

* Good, because 上の Consequences のとおり。
* Bad, because ADR 20 のサーバーに、ホストとサンドボックスという二つの動かし方ができる。承認の理由と警告で、どちらで動いているかを示す必要がある。

### プラグインの仕組みを別に作る

* Good, because 配布の単位 (ディレクトリ) と、呼び出しごとの起動による強い隔離を持てる。
* Bad, because ADR 20 と別に、外部のコードを呼ぶ経路、manifest、配置の検査、プロトコルの規約を持つことになる。ADR 20 の設定だけで、利用者が決めるという性質はもう満たせる。

### hook を中心にしたプラグイン

* Good, because Claude Code の hooks の考え方をそのまま使える。
* Bad, because allow を返す hook や引数を書き換える hook は、承認の不変条件を壊す。拒否だけに限ると、`approval.deny` との違いがほとんど残らない。
* Bad, because 主な用途 (編集後のフォーマット、通知、ログ) は、`/undo` の保証と両立しないか、サンドボックスの外に出る必要があるか、ADR 4 と ADR 17 の記録で取れる。

### 同じプロセスで動くプラグイン

* Good, because 速く、内部の状態に直接触れられる。
* Bad, because dylib は Rust の ABI が安定しておらず、プロセスの権限がすべて渡る。
* Bad, because 組み込みの JS や WASM は、依存と、socium が公開して保守する API の面積が大きい。WASM を除けば、サンドボックスの外で任意のコードが動く経路にもなる。

## More Information

ADR 20 (MCP サーバーとその承認)、ADR 21 (遠隔のサーバー)、ADR 23 (サブエージェントの MCP の tool)、ADR 12 (サンドボックスを広げる口)、ADR 13 (承認)。SOC-036 (追加拡張の採否)、SOC-025 (`find_symbol` の限界)、SOC-032 (評価)。OpenCode のプラグインは https://opencode.ai/docs/build/plugins/server 、Claude Code のプラグインは https://code.claude.com/docs/en/plugins-reference による。

この ADR の前に、MCP とは別のプラグインの仕組みを設計し、Codex (gpt-6-astra) のレビューを 2 回受けた。そのレビューで決まったことのうち、次のものをここに引き継いだ。

* `sandbox.read_only`、`path`、`env` を継承しない。
* 制限の計算に失敗したら、承認や制限なしの実行に切り替えずに止める。
* 秘密の保証を、実際に隠せる範囲に限り、例外を明示する。
* 組み込みの tool を比較の基準に置き、閾値と、何も入れない場合の結論を先に決める。
* ast-grep の `paths` と出力の上限。

このレビューで、既存の問題も見つかった。

* `sandbox.env` は `LD_PRELOAD` などのローダーの変数を拒否していない (`settings.rs` の `reserved_variable`)。
* その値は、サンドボックスを作る前の bubblewrap 自身の環境に入る (`execution/linux.rs` の `command`)。
* 2026-11-19 に `file` で確かめたところ、flake の bubblewrap (`/nix/store/3hqwbwxij9lxhyrf2amsm2xg24xszmmx-bubblewrap-0.12.0/bin/bwrap`) は動的リンクされていた。

この ADR の `env` は `--setenv` で渡すので、この問題の影響を受けない。`sandbox.env` のほうは PR #100 で、同じく記述子から `--setenv` で渡すように直す。
