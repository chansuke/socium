---
number: 11
title: socketpairを接続済みのstreamとseqpacketに限って許可する
status: proposed
date: 2026-11-15
---

# socketpairを接続済みのstreamとseqpacketに限って許可する

## Context and Problem Statement

サンドボックスの seccomp は `socketpair` を型を問わず EPERM にしていた。理由は、Unix datagram の対が `sendto` でホストの pathname socket に届くことだった。ところが Rust の `std::process`、Python の `asyncio`、Node の `child_process` は子プロセスの起動に `socketpair` を使うので、サンドボックスの中で `cargo build`・`rustc`・asyncio を使う pytest・`execSync` が失敗し、編集の後に検証できなかった (2026-11-14 のレビュー N1、再現 R6)。対照として `SYS_socketpair` の 1 行を拒否リストから外したコピーでは、4 つとも成功した。

## Decision Drivers

* サンドボックスの中で実際のツールチェーンが動く。
* ネットワークとホストの Unix socket へ届く経路を開けない。
* seccomp で検査できる範囲で決める。

## Considered Options

* `socketpair` の第 2 引数 (型) を見て、`SOCK_STREAM` と `SOCK_SEQPACKET` だけ許可する
* Codex CLI のように `AF_UNIX` の `socket`・`socketpair` を許可し、`connect`・`bind`・`sendto` を拒否する
* 拒否のまま、文書に制限を書く

## Decision Outcome

Chosen option: 「`SOCK_STREAM` と `SOCK_SEQPACKET` だけ許可する」。接続済みの stream と seqpacket の対はアドレスを持たず、相手は自分の対だけなので、`socket()` を塞いだままなら外へ出られない。datagram の対だけが `sendto` で宛先を選べるので、その型を拒否し続ければ元の理由は残らない。

* BPF は `socketpair` の呼び出しで `args[1]` の下位 32 ビット (offset 24) を読み、`SOCK_CLOEXEC` と `SOCK_NONBLOCK` を `& 0xf` で落としてから比べる。stream と seqpacket は許可し、それ以外は EPERM を返す。
* `socket`、io_uring、その他の拒否はそのまま。
* `cargo run --offline`、Python の asyncio の子プロセス、Node の `execSync` がサンドボックス内で動くことを受け入れテストにする。テストが使う `python3` と `nodejs` を flake の検査用入力と開発シェルに入れる。

### Consequences

* Good, because 編集→ビルド→テストの検証ループがサンドボックスの中で回る。
* Good, because 許可するのは相手が固定の対だけで、ネットワークもホストの socket も閉じたまま。
* Bad, because 型の値で判断するので、将来 Unix socket の型が増えたら見直しが要る。今 Unix socket で使える型は stream、datagram、seqpacket だけ。

### Confirmation

`execution/tests.rs` の `network_probe` で、stream と seqpacket の対が (フラグ付きでも) 作れ、datagram の対は拒否され、ホストの pathname socket には接続できないことを確かめる。`real_toolchains_build_and_spawn_processes_inside_the_sandbox` で、Cargo のプロジェクトのビルドと実行、asyncio の子プロセス、Node の `execSync` がサンドボックス内で成功することを確かめる。`nix build` の検査でも同じテストが通る。

## Pros and Cons of the Options

### 型で絞って許可する

* Good, because 変更が seccomp の数命令で済む。
* Good, because datagram を閉じたままなので、元の懸念に答えている。
* Neutral, because 型の値に依存する。

### Codex CLI の方式 (`AF_UNIX` を許可し、`connect` などを拒否)

[codex-rs/linux-sandbox/src/seccomp.rs](https://github.com/openai/codex/blob/b1573f102413e96a6481087922bf425231222dee/codex-rs/linux-sandbox/src/seccomp.rs#L256-L274) はこの形で、コメントに `cargo clippy` が socketpair を使うとある。

* Good, because 実績がある。
* Bad, because `sendmsg` の宛先は seccomp で検査できず、datagram の対からホストの socket へ届く経路を別の手段で塞ぐ必要がある。

### 拒否のまま文書に書く

* Good, because 変更が無い。
* Bad, because Rust のプロジェクトをビルドもテストもできず、主用途が成立しない。

## More Information

2026-11-14 のレビュー `docs/review-socium-2026-11-14.md` の N1。サンドボックスの方針は [Linux sandbox guide](../linux-sandbox.md)。
