---
number: 20
title: --dangerously-skip-permissionsで承認をすべて省く実行を起動の引数に限って許す
status: proposed
date: 2026-11-20
links:
- target: 13
  kind: amends
---

# --dangerously-skip-permissionsで承認をすべて省く実行を起動の引数に限って許す

## Context and Problem Statement

ADR 13 では、`approval.commands = "auto"` でもコミット、ホストが実行するファイル (`approval.protected`) の編集、秘密を含みうるファイル (`approval.sensitive`) の読み取りは必ず尋ねる。`-p` では尋ねる相手がいないため、これらは拒否される。使い捨てのコンテナや CI のように、利用者が取り消せない結果を受け入れたうえで最後まで任せたい場面では、承認が作業を止める。Claude Code は `--dangerously-skip-permissions` (`--permission-mode bypassPermissions`) で承認を省き、保護されたパスへの書き込みも尋ねない ([permission modes](https://code.claude.com/docs/en/permissions#permission-modes))。承認を省く実行を socium に入れるか、入れるなら何を残すかを決める。

## Decision Drivers

* 利用者が選んだときに、承認で作業が止まらない。
* モデルがこの実行に切り替えられない (ADR 12)。
* 利用者が自分で書いた拒否は守る。
* 承認を省いていることが、起動時と画面で分かる。

## Considered Options

* 起動の引数 `--dangerously-skip-permissions` で、承認をすべて省く
* `approval.commands` に `"bypass"` を足し、`config.toml` で選ぶ
* 入れない

## Decision Outcome

Chosen option: 「起動の引数で承認をすべて省く」。名前は `claude` の引数に揃え、`claude` を使い慣れた人が同じ書き方で使えるようにする。

* `--dangerously-skip-permissions` は TUI と `-p` の両方で受け付け、そのプロセスの間だけ効く。設定ファイルからは選べない。
* この実行では、`run_command`、`git_commit`、保護されたファイルの編集、秘密を含みうるファイルの読み取りを尋ねずに実行する。尋ねずに実行するコマンドに ADR 13 で掛ける狭いサンドボックス (秘密を空にし、保護されたファイルを読み取り専用にする) も掛けない。
* `approval.deny` に一致するコマンドは、この実行でも拒否する。
* サンドボックスは変えない。承認はサンドボックスを広げない (ADR 12、ADR 13)。
* 起動時の notice に省いていることを出し、TUI の footer に赤で `approvals skipped` と出す。activity log の `tool_started` の `authorization` は `approvals-skipped` にする。
* サブエージェントは同じ設定で動くので、同じく尋ねない。

### Consequences

* Good, because `-p` でコミットや `build.rs` の編集まで任せられる。
* Good, because モデルは引数を変えられないので、この実行に切り替えられない。
* Bad, because モデルが `.envrc` や `build.rs` のようなホストが後で実行するファイルを書き換えられる。その後でホストの道具が動くと、サンドボックスの外で実行される。使い捨ての環境で使う。
* Bad, because 秘密を含みうるファイルの中身がモデルの送り先に渡りうる。

### Confirmation

`approval.rs` の単体テストで、この実行ではコマンド、コミット、保護されたファイルの作成、`.env` の読み取りが制限なしに実行され、`approval.deny` に一致するコマンドは拒否されることを確かめる。`cli.rs` の単体テストで、引数が `-p` の有無を問わず受け付けられることを確かめる。TUI の crate の統合テストで、実際のバイナリを `-p --dangerously-skip-permissions` で起動し、承認が要る `build.rs` の作成が実行され、stderr に notice が出ることを確かめる。footer の表示は view のテストで確かめる。

## Pros and Cons of the Options

### 起動の引数で承認をすべて省く

* Good, because 起動のたびに選ぶので、意図せず残らない。
* Good, because `claude` と同じ名前で使える。
* Bad, because 毎回引数を書く必要がある。

### `approval.commands` に `"bypass"` を足す

* Good, because 一度書けば済む。
* Bad, because 設定が残り、使い捨てでない環境でも黙って効き続ける。
* Bad, because `approval.commands` はコマンドの扱いを決める項目で、コミットや読み取りまで含める名前ではない。

### 入れない

* Good, because 保護されたファイルの編集と秘密の読み取りが必ず人の目を通る。
* Bad, because `-p` ではコミットや保護されたファイルの編集を任せる方法がない。

## More Information

[ADR 13](0013-承認を取り消しにくい操作に絞りセッション単位の許可を持つ.md) を改める。Claude Code の bypass で `permissions.deny` が効くかは、[permissions](https://code.claude.com/docs/en/permissions) からは読み取れなかった (未確認)。socium では利用者が自分で書いた拒否なので残す。
