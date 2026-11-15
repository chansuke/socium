# socium 総合レビュー (2026-11-14)

対象: socium `2d4ac914051e974efa24b926d96e9b42e97904d5` (main)。実装は変更していない。追加したのはこの文書と `docs/review-evidence/2026-11-14/` だけである。

## 1. 結論

前回レビュー (F1〜F12) が指摘した「承認・パス境界・プロセス・記録・stream・patch・commit・中断・保存」の欠陥は、今回すべて現行コードで再検証した。F1〜F11 が指摘した中心の欠陥は解消されている (残りと修正の副作用は 4 章)。承認はランタイムが所有し、コマンドは fail-closed の OS サンドボックスで動き、実行済み操作は逐次保存され、未完了の操作は再実行されない。目標 3 (既存の変更を壊さない) を支える部分は、比較した pi / DSH と比べても socium のほうが厳格で、ここは直してはいけない。

一方で、その厳格さの上で **日常の作業ループが成立していない**。今回の指摘の中心は次の 3 点で、いずれも再現済みである。

1. サンドボックスの中で Rust がビルドできない。seccomp が `socketpair` を型を問わず拒否するため、`cargo` は `rustc` を、`rustc` はリンカを起動できない。Python の `asyncio` と Node の `child_process` も同じ理由で動かない。socium は自分自身をビルドもテストもできない。実利用のログにも同じ失敗が残っている (N1)。
2. モデルへの HTTP リクエストが、流れ続けているストリームでも 90 秒で必ず切れる。実測の出力速度 (約 107 トークン/秒) では約 1 万トークンで、既定の `MAX_TOKENS` 64,000 の 15% にあたる (N2)。
3. 1 ターンで扱える情報量が推定 13.6 万トークンで頭打ちになり、超えるとターンが失敗する。その後の送信は 7 回続けて失敗し、復帰したときには読んだ内容が 372 文字の機械的な要約に置き換わっている。上限は組み込みモデルの窓を 20 万トークンと見なす既定値から来ているが、Anthropic の現行資料では 100 万である (N3)。

前回の枠組みは「壊さない・失わない」に寄っていて、「サンドボックスの中で実際の開発ツールが動くか」「長い応答と長いターンが通るか」を誰も実行して確かめていなかった。テストは 409 件あるが、サンドボックス内で動かしているのは `sh`・`sed`・`env`・`git` で、コンパイラもテストランナーも走らせていない。目標 1 (導入の容易さ) と目標 4 (止めて再開) の未達は、この検証の空白から来ている。

**最初にやるべきこと (作業順)**

| 順 | 作業 | 理由 | 指摘 |
|---|---|---|---|
| 1 | seccomp の `socketpair` を型で絞り、サンドボックス内で `cargo build`・`asyncio`・Node の子プロセスが通ることを受け入れテストにする | 編集→検証のループが成立しないと、他の改善の効果も測れない。変更は数行 | N1 |
| 2 | HTTP の全体期限 90 秒を、接続と無通信の期限に置き換える | 長い応答が必ず失敗し、再試行しても同じ結果になる。変更は数行 | N2 |
| 3 | 組み込みモデルの窓を実値にし、巨大な tool 結果を送信時に縮め、ループ内で圧縮する | 長いターンの失敗と、復帰時の文脈喪失をなくす | N3、N7 |
| 4 | Anthropic 形式のリクエストに `cache_control` を付ける | 実ログで Opus の入力 688,901 トークンが全額課金。履歴は既に追記専用で、条件は整っている | N4 |
| 5 | サンドボックスをユーザー設定で広げる口を決める (依存キャッシュの読み取り専用マウント、PATH、時間上限、新規ファイルのコミット、worktree) | 1 を直しても、crates.io に依存するプロジェクトはビルドできない。プロダクトの方向性の判断が要る | N5、8 章 |

深刻度は依頼の定義 (P1 = 目標 3・4 や安全性を通常利用で損なう) に従った。N1 と N2 は定義の文言には当たらないが、通常利用で中核機能が成立しないため **P1\*** と表記して P1 と同列に並べた。定義どおりに読むなら P2 である。

## 2. 検証範囲と限界

**固定したコミット**

| 対象 | コミット | 用途 |
|---|---|---|
| socium (開始時) | `ddb44dfbf658` | 開始時点の `main`。コード読解と最初の再現 |
| socium (中間) | `c3b8790af052` | 作業中に 8 コミット進んだ (セッション、`-c`/`-r`、crane)。全再現をやり直した |
| socium (判定対象) | `2d4ac914051e` | さらに 8 コミット進んだ (subagent)。全再現をもう一度やり直した。本文の行番号と判定はこのコミットのもの。書き終えた時点の `main` は `5c9194b` で、差は `flake.nix` の 4 行だけ (Rust と文書は同一) |
| pi | `c1d4b0ac3e35` (前回 `682c7b93ce3e` から 176 コミット) | `packages/coding-agent`・`ai`・`tui` の該当箇所 |
| DSH | `39163dd30f70` (前回 `fcc0ad462172` から 1,371 コミット) | `packages/sandbox`・`interaction`・`compaction` の README と `profiles.ts` |
| Codex CLI | `b1573f102413` | `codex-rs/linux-sandbox/src/seccomp.rs` のみ |

**実行したもの** (ログは `docs/review-evidence/2026-11-14/`)

- 既存テスト: `ddb44df` で core 255 件・TUI 79 件、`c3b8790` で core 283 件・TUI 92 件、`2d4ac91` で core 311 件・TUI 98 件がすべて成功 (ほかに有料 API のテスト 5〜6 件と目視用 1 件が ignored)。
- 再現テスト 15 件 (`review_repros.rs`。本文では R1〜R16 と呼ぶ。対応は 9 章)。一時ディレクトリへ `git archive` で展開したコピーに追加して実行した。本体の作業ツリーには何も書いていない。3 つのコミットのどれでも同じ結果になった。
- 作業中に入ったセッション (ADR 7) と subagent (ADR 8〜10) は、ADR と差分を読み、既存テストと再現を回し直して既存の指摘への影響を確認した。この 2 つを対象にした新しい再現テストは書いていない。
- 対照実験 1 件: seccomp の拒否リストから `SYS_socketpair` の 1 行だけを除いたコピーで再現 R6 を再実行。
- 実利用の `.socium/` を読み取り専用で集計 (イベント種別、ツール成否、承認の待ち時間、停止理由、トークン数、ファイルサイズ)。会話本文は引用していない。
- ホストの事実確認: AppArmor のプロファイル、`bwrap`・`node`・`python3` の所在と版。
- tmux 上の実端末で TUI に複数行を貼り付け (ローカルの無応答サーバーを接続先にし、外部へは送信していない)。

**実行しなかったもの**

- 実 API への送信。したがって「互換 endpoint が他モデルの thinking を拒否するか」「Opus 5.5 の実際の窓」「`cache_control` を付けたときの実際の課金」は未確認で、該当箇所にそう書いた。
- pi / DSH / Codex のビルドと実行。比較はソースと README の読解で、「実装で確認」と「README の記述」を区別した。
- macOS、aarch64、Ubuntu 以外のディストリビューション。
- `analystic_target/`。`.gitignore` と `docs/public-release-review.md` から「公開対象外のローカル調査用チェックアウト」と読めるが、中身は読んでおらず、位置付けは不明として評価していない。

**外部仕様の出典**

- ストリーミング: <https://platform.claude.com/docs/en/build-with-claude/streaming> (2026-11-14 に取得)。
- thinking の再送規則、プロンプトキャッシュ、モデルの窓と価格、text editor tool の版: Claude Code 同梱の claude-api リファレンス (`shared/model-migration.md` ほか、キャッシュ日 2026-10-06)。公式サイトの本文とは突き合わせていない (未確認)。thinking と履歴編集の規則の一次資料は <https://platform.claude.com/docs/en/build-with-claude/preserved-thinking/prefix-checks> だが、今回は取得していない。

## 3. 維持すべき良い点

直すときに壊してはいけないもの。根拠は現行コードと、成功を確認した既存テスト。

| 良い点 | 根拠 | なぜ守るか |
|---|---|---|
| 承認をランタイムが所有する | `crates/core/src/control.rs:40-98`、`crates/core/src/tools/mod.rs:221-247`。モデル引数の `approval` は無視され、承認経路がなければ拒否 (`forged_approval_is_denied_and_human_approval_is_single_use`) | F1 の再発防止。承認を緩める変更 (8 章) をしても、この所有関係は変えない |
| サンドボックスが fail-closed | `crates/core/src/execution/linux.rs:97-239`。環境変数は許可リストで再構築、`.socium` は隠し、`.git` は commit 以外で読み取り専用、起動時に実プローブ (`crates/core/src/lib.rs` の `from_env`) | 比較対象より厳格。pi は隔離なし、DSH の Linux プロファイルはホスト全体を読み取り可で見せる (6 章) |
| 副作用の前に意図を保存し、結果不明は再実行しない | `crates/core/src/agent/turn.rs:165-168,367-370,403`、`repair_incomplete_tools` (`turn.rs:523`) | 目標 4 の土台。N3 の修正でターンの途中に圧縮を入れるときも、この順序を崩さない |
| 原子的な保存と単一 writer | `crates/core/src/agent/state.rs:272-329,331-364`。保存境界への故障注入テストあり | 壊れた状態から空の会話で上書きしない |
| stream の完了を検証してから tool を解放 | `crates/core/src/providers/anthropic/stream.rs:263-323`、他 2 形式も同じ規約 | N11 で未知イベントを許容するときも、tool 解放の条件は緩めない |
| patch は全検証の後に書く | `crates/core/src/tools/apply_patch.rs:138-233`、`crates/core/src/tools/edit_history.rs:94-151`。曖昧な hunk を拒否、行ごとの改行と mode を保持 | 部分適用を作らない |
| commit の範囲を限定し、承認中の変化を検出 | `crates/core/src/tools/git.rs:359-395,397-462`、`tools/mod.rs:236-243` | ユーザーの index を壊さない |
| 要約後に残したターンの thinking を送らない | `crates/core/src/agent/context.rs:253-263`、`thinking_valid_from`。claude-api リファレンスが keep-tail 型の圧縮に求める形 (保持ターンの thinking を外す) と一致 | 新しいアカウントでは履歴編集後の thinking 再送が 400 になる。履歴が追記専用なのでキャッシュにも向く |
| 接続先の登録が安全側 | `crates/core/src/models.rs:362-431`。キーは環境変数名だけ、HTTPS 強制、リダイレクト禁止、エラー本文は整形して制御文字を除去 (`providers/error.rs:44-91`) | 設定の誤りでキーが漏れない |
| Activity の構造化イベント | `crates/core/src/activity.rs`、`activity_log.rs`。TTFT・停止理由・トークン・承認の待ち時間が round 単位で残る | 今回の実利用の集計はこの記録だけでできた。評価基盤 (N14) の素材になる |
| subagent が既存の保証を迂回しない | 同期実行で、権限は渡した tool の一覧で強制し (`turn.rs` の `offered` の判定)、入れ子は不可。利用者定義の種類は設定ディレクトリからだけ読む (`agent/subagent/catalog.rs:428-438`、ADR 9) | リポジトリの内容が種類や権限を足せない。N8 の境界と整合している |
| 小さい | Rust 約 29,400 行 (テスト込み)。pi は `coding-agent/src` だけで約 75,000 行 | 所有者 1 人で全体を把握できる規模を保つ |

## 4. 前回レビューと SOC チケットの追跡

判定: (a) 解消済み、(b) 部分的に解消、(c) 未解消、(d) 修正が新しい問題を生んだ、(e) 指摘が誤っていた。(e) に当たるものは無かった。

### F1〜F12

| ID | 判定 | 根拠 (現行コードと実行結果) | 残り・副作用 |
|---|---|---|---|
| F1 承認 | (a) | `tools/mod.rs:221-247`。再現 R16 で、承認経路なしの `run_command` は拒否 | 承認の運用に別の問題 (N9) |
| F2 新規パス | (a) | `tools/workspace.rs:69-125`。`create_normalizes_missing_components_without_side_effects` が旧再現を反転 | 並行する symlink 差し替えは文書どおり対象外 |
| F3 実行権限 | (a)+(d) | `execution/tests.rs` の 9 件が成功 | 隔離の副作用で開発ツールが動かない (N1、N5) |
| F4 プロセス | (a) | `process.rs:120-155`。両 pipe 2 MiB、子孫、signal のテストが成功 | モデルに渡る出力は先頭 20,000 文字のまま (N12) |
| F5 記録 | (a) | `turn.rs:167,369,403`。再起動をまたぐ受け入れテストが成功 | チェックポイントごとに全体を書き直す (N15) |
| F6 会話契約 | (a)+(b) | 収まる履歴は要約しない (`short_history_that_fits_is_not_summarized`)、要約は user role、tool の対を保持 | 予算超過はターンの失敗になる (N3) |
| F7 stream | (a)+(d) | `stream.rs:263-293` | 厳格にした結果、未知のイベント種別で応答全体が失敗する (N11) |
| F8 patch | (a)+(d) | 計画の共有、曖昧 hunk の拒否 | dry-run が変更前後の全文を返す (N12)。undo は到達経路がない (N6) |
| F9 commit | (a) | `git add -A` は無い。scoped commit のテストが成功 | 新規ファイルをコミットできない (N5) |
| F10 中断 | (a) | `crates/tui/src/app.rs:530-540`、HTTP・承認・コマンドの中断テストが成功 | 実行中の入力 (SOC-035) は (c) |
| F11 保存 | (a)+(b) | temp→sync→rename、OS ロック、schema version | 追記ログではなく全体書き直し。edit history に上限到達後の出口がない (N6) |
| F12 Provider | (b) | 3 形式を実装。変換は各 adapter 内で、core の型は Anthropic 形のまま | 生成元の記録がない (N10) |
| F12 能力情報 | (b) | `features` を明示 (`models.rs:71-104`) | 組み込みモデルの窓が未設定で 20 万扱い (N3) |
| F12 Context 予算 | (c) | 文字数÷3.5 (`context.rs:427-435`)、ループ内は判定のみ (`turn.rs:76-89`) | N3 |
| F12 要約品質 | (b) | 追記時に新しい修正を残す (`741f023`) | 入力が 1 メッセージ 600 文字 (N7) |
| F12 API 障害 | (c) | retry・backoff は無い | 加えて 90 秒の全体期限 (N2) |
| F12 使用量・cache | (c) | round ごとの usage は Activity に出る。累積、費用、`cache_control` は無い | N4 |
| F12 Prompt | (c) | システムプロンプトは 3 文 (`constants.rs:39`)、指示ファイルの読込なし | N12 |
| F12 tool 出力 | (c) | 文字列のまま | N12 |
| F12 検索 | (a) | `rg --json`、`-e`、期限、打ち切りの明示 (`tools/ripgrep.rs`) | 1 行の長さに上限がない (N12) |
| F12 Session | (a) | `5ab8044` 以降でセッション、`-c`/`-r`、tool 履歴の再表示 (`session.rs`、`transcript.rs`) | — |
| F12 UI・文字幅 | (b) | Activity 欄、表示幅での折り返し (`c33c0f4`) | 貼り付け、差分表示 (N13) |
| F12 配布 | (b) | Nix の wrapper が git・rg・bwrap を同梱 | Nix 以外では bwrap 0.12 以上を自分で用意する。リリース物は無い (N5) |
| F12 品質基盤 | (b) | ScriptedProvider、ローカル HTTP、故障注入 | headless と評価がない (N14) |

### SOC-001〜042

GitHub の状態は 2026-11-14 に `gh issue list` で取得した。「実装」は現行コードでの判定。

| チケット | GitHub | 実装 | 根拠・注記 |
|---|---|---|---|
| SOC-001、002、004、005、006、007、008、010、011、012、024、038、039 | closed | (a) | 上の F1〜F11 と同じ根拠。SOC-006 は予算超過の扱いに (b) が残る |
| SOC-003 | closed | (a)+(d) | N1、N5 |
| SOC-009 | closed | (b) | 記録と undo の実装はあるが、呼び出す経路がない (N6) |
| SOC-013 | open | (a) 相当 | 生履歴を先頭から切らなくなったので、ずれる前提が消えた。Issue は閉じられる状態 |
| SOC-014 | open | (c) | core の型は Anthropic 形。ADR 6 も先送りを明記 |
| SOC-015 | open | (b) | registry はある。`CONTEXT_WINDOW_TOKENS` などは `env::var` を直接読む (7 章) |
| SOC-016 | **open** | **(a)** | `fd143b8` で実装済み。Issue とチケットの「状態: 未着手」が古い |
| SOC-017 | open | (c) | N3 |
| SOC-018 | open | (b) | N7 |
| SOC-019 | open | (c) | N2、N11 |
| SOC-020 | open | (b) | round 単位の usage は記録。累積と費用は無い |
| SOC-021 | open | (c) | N4 |
| SOC-022 | open | (c) | N12 |
| SOC-023 | open | (c) | N12 |
| SOC-025 | open | (b) | 説明文は Rust 専用と明記。他言語は無い |
| SOC-026 | closed | (a) | `5ab8044` 以降で実装。セッションのテストを含む既存テストの成功を確認 |
| SOC-027 | open | (b) | Activity 欄は実装。コマンド出力は先頭優先のまま |
| SOC-028 | **open** | **(a)** | `c33c0f4`、`ea938fe` で実装済み。Issue が古い |
| SOC-029 | open | (b) | N5 |
| SOC-030 | open | (b) | N14 |
| SOC-031、032、033 | open | (c) | N14 |
| SOC-034 | closed | (b) | その後の変更で `docs/todo.md` などが再びずれた (7 章) |
| SOC-035 | open | (c) | 実行中は入力を受け付けない |
| SOC-036、037 | open | — | 採否判断と未実装の機能 |
| SOC-040 | **open** | **(a)** | `336270d` で実装済み。Issue と `docs/daily-reliability.md:44` が古い |
| SOC-041、042 | closed | (a) | N10 に残課題 |

## 5. 新規の指摘

### N1 (P1\*) サンドボックス内で Rust・Python asyncio・Node の子プロセスが動かない

- **主張**: seccomp が `socketpair` を型を問わず EPERM にするため (`crates/core/src/execution/linux.rs:270`)、Rust 標準ライブラリの子プロセス起動、Python の `asyncio`、Node の `child_process` がサンドボックス内で失敗し、Rust プロジェクトは socium 自身を含めてビルドもテストもできない。
- **根拠**: 再現 R6 (`repros.log`)。`rustc main.rs` は `could not exec the linker cc … Operation not permitted (os error 1)`、依存のない `cargo build` は `could not execute process rustc … (never executed) … Operation not permitted`、`asyncio.run` はイベントループの生成で例外、Node の `execSync` は `spawnSync /bin/sh EPERM`。同じサンドボックスで `cc t.c` と Python の `subprocess` は成功する。対照として `SYS_socketpair` の 1 行だけを除いたコピーでは 4 つとも成功した (`repro-r6-socketpair-allowed.log`、`control-socketpair.diff`)。実利用でも `.socium/outputs/` の 2026-11-13 の stderr に同じ `rustc` のエラーが残っている。Activity の集計では `run_command` 8 回のうち 4 回が失敗で、内訳は承認されなかったもの 2 回、このエラー 1 回、別の原因 1 回 (`real-usage-aggregates.txt`)。
- **成立条件と影響**: Linux で socium を使う全員。`run_command` で `cargo build/test/clippy`、`asyncio` を使う pytest、子プロセスの出力を読む npm スクリプトを実行したとき。編集後の検証ができず、モデルは失敗を見て迂回を試み、ラウンドと承認を消費する。
- **確度**: 確認済み (このホストの Nix 提供 Rust 1.92.0、Python 3.13、Node 24。ディストリビューション提供のツールチェーンでは未確認)。原因の同定は対照実験による。Rust 標準ライブラリのどの経路が `socketpair` を使うかはソースで確かめていない (推測: `posix_spawn` に乗らない起動で、エラー通知用に `AF_UNIX`/`SOCK_SEQPACKET` の対を作る)。`rustc --version` のような単体の起動は成功する。
- **検討した反論**: (1) 意図した制限ではないか。`docs/linux-sandbox.md:81-84` は「socketpair を使うローカル IPC は未対応」と書くが、理由は「socketpair の Unix datagram がホストの pathname socket に届く」ことで、これは `SOCK_DGRAM` にしか当てはまらない。`SOCK_STREAM` と `SOCK_SEQPACKET` の対は相手が固定で、`socket()` を塞いでいれば外へ出られない。文書は cargo や asyncio が該当することにも触れていない。(2) 他の実装はどうか。Codex CLI は同じ「ネットワーク制限」モードで `AF_UNIX` の `socket`/`socketpair` を許し、`connect`・`bind`・`sendto` などを拒否している。コメントに `cargo clippy` が socketpair を使うとある ([seccomp.rs:256-274](https://github.com/openai/codex/blob/b1573f102413e96a6481087922bf425231222dee/codex-rs/linux-sandbox/src/seccomp.rs#L256-L274))。
- **推奨**: seccomp で `socketpair` の第 2 引数 (type) を見て、`SOCK_STREAM` と `SOCK_SEQPACKET` を許可し、`SOCK_DGRAM` は拒否のままにする。あわせて「サンドボックス内で `cargo build`、`asyncio.run`、Node の `execSync` が通る」を `execution/tests.rs` の受け入れ条件に加える。取らなかった案: Codex 型 (socketpair を全許可して `connect`/`sendto` を拒否)。`sendmsg` の宛先は seccomp で検査できないので、datagram を閉じるほうが単純。文書に制限を追記するだけの案は、主用途が成立しないので採らない。

### N2 (P1\*) 流れているストリームでも 90 秒で切れる

- **主張**: HTTP クライアントの 90 秒は応答本文の完了までを含む全体の期限なので、90 秒を超える応答は内容にかかわらず失敗する (`crates/core/src/lib.rs:262-267`)。
- **根拠**: 再現 R1 (`repro-r1-http-timeout.log`)。毎秒 1 トークンを 120 秒流す健全な SSE が 90.02 秒で切れ、89 個の delta を受け取ったあと `Http(reqwest::Error { kind: Decode, source: … TimedOut })` になった。利用者に見える文言は `error decoding response body` で、期限切れとは読めない。実利用のログでは Opus の最長ラウンドが出力 5,216 トークンで 48.5 秒 (約 107 トークン/秒) だった。この速度だと 90 秒は約 9,600 トークンで、既定の `MAX_TOKENS` 64,000 の 15% にあたる。
- **成立条件と影響**: thinking が長い応答、数百行のファイルを 1 回の tool 入力で書く応答、effort を上げたとき。再試行しても同じ結果になる。失敗した応答は履歴に入らず (`turn.rs:127` で打ち切り)、課金済みの出力は捨てられる。要約の呼び出しも同じクライアントを使う。
- **確度**: 確認済み。
- **検討した反論**: 実利用のログに期限切れは 0 件 (最長 48.5 秒) で、まだ踏んでいないだけである。claude-api リファレンスは「難しいタスクの 1 リクエストは数分かかる」とし、SDK の既定の期限は 10 分。
- **推奨**: 全体期限をやめ、接続の期限と無通信の期限 (チャンク間) に分ける。エラーは「N 秒間応答がなかった」と表示する。取らなかった案: 90 を大きい数に変えるだけ (上限のある応答長という同じ形の欠陥が残る)。

### N3 (P1) 1 ターンの情報量が推定 13.6 万トークンで頭打ちになり、超えると会話が続けられない

- **主張**: 入力予算を超える tool 結果が 1 ターンに入ると、そのターンは失敗し、以後の送信も連続して失敗し、復帰時には読んだ内容が失われる。上限は実際のモデルの窓より大幅に低い既定値から来ている。
- **根拠**: 再現 R3 (`repros.log`)。5 往復の履歴があるセッションで、199 KB のファイル 3 つを 1 ラウンドで読むと、(1) 次のラウンドの前に `Context budget exceeded during tool loop; compact before continuing` で失敗 (`turn.rs:76-89`。読んだ内容はモデルに一度も渡らない)、(2) `/compact` は `Nothing to compact` (直近 16 メッセージは対象外: `constants.rs:33`)、(3) 次の送信は `Context exceeds input budget even after compaction; shorten input or increase CONTEXT_WINDOW_TOKENS` で失敗し、**7 回** 続く (末尾 8 メッセージは必ず残す: `context.rs:208-229`。失敗のたびにユーザー発言が 1 件ずつ履歴に積まれ、巨大なメッセージが 8 件より前に押し出されるまで続く)、(4) 復帰後のリクエストは 8,244 文字で、ファイルの本文は無く、372 文字の機械的な要約に置き換わっている。同じ依頼を出せば同じ読み込みで同じ失敗になる。
- **根本原因と派生**:
  - 組み込みの `opus`・`sonnet`・`haiku` は `context_window: None` (`models.rs:193`) で、既定の 200,000 (`constants.rs:19`) が使われる。出力枠 64,000 を引いた 136,000 が入力予算になる。claude-api リファレンスのモデル表では 3 つとも窓は 100 万トークン (実 API の Models API では未確認)。自動圧縮の閾値は予算の 80% = 108,800 で、実際の窓の約 11% で圧縮が始まる。
  - ループ内は判定だけで、圧縮も縮約もしない (`turn.rs:76-89`)。自動圧縮はターンの間にしか走らず、直前の実測 usage が無い再起動直後は走らない (`lib.rs:1158-1165`)。
  - `read_file` は範囲指定がなく、最大 200 KB を丸ごと返す (`tools/read_file.rs:61-74`)。
  - 推定は直列化した文字数÷3.5 (`context.rs:415-436`)。日本語では過小になるはずだが、実 API の `count_tokens` と比べていない (推測)。
  - エラー文が案内する `/compact` は効かず、`CONTEXT_WINDOW_TOKENS` は README に無い。
- **成立条件と影響**: 既定のモデルで、1 ターンに合計 48 万文字前後の tool 結果が入ったとき。大きいファイル 3 つの並列読み込みのほか、このリポジトリなら主要ソース 10 本強を読む調査でも届く。編集は保存されるが、読んだ内容と作業の流れは失われ、送信が何度も空振りする。
- **subagent との関係**: `2d4ac91` で入った `explorer` に調査を任せれば、親の文脈に入るのは報告 (最大 20,000 文字) だけなので、この上限には届きにくくなる。ただし子も同じ予算、同じ 20 ラウンド、同じ 90 秒 (N2) で動き、任せるかどうかはモデルの判断である。上限そのものは変わっていない。
- **深刻度**: P1 (目標 4)。**確度**: 確認済み (窓の実値のみ未確認)。
- **検討した反論**: 最初は「セッションが恒久的に詰まる」と仮定したが、再現で否定された。履歴が 8 メッセージ以下なら次の送信で復帰し、それより長くても 7 回の失敗の後に復帰する。`models.toml` で窓を 100 万と書いたモデルではほぼ起きない。したがって最小の緩和は組み込み 3 モデルの窓を実値にすることで、それだけで頻度は大きく下がる。ただし形は残る。
- **推奨**: (1) 組み込みモデルに実際の窓を持たせる。(2) 送信時の投影で、古い・巨大な tool 結果を先頭と末尾だけに縮める (生履歴は残す)。DSH の tool-result-pruner と同じ形で、socium の「生履歴は保持し、投影だけ変える」方針に合う。投影を変えた時点より前の thinking は送れなくなるので、既存の `thinking_valid_from` に乗せる。(3) ループ内で閾値を超えたら、次のリクエストの前に同期で圧縮する。(4) 各ラウンドの実測 usage (`turn.rs:128-130`) で推定を補正する。(5) エラー文を実際に効く操作に直す。取らなかった案: Anthropic のサーバー側 compaction・context editing。3 形式で同じ挙動にできないので、Anthropic 形式だけの最適化に留める。

### N4 (P2) Anthropic 形式でプロンプトキャッシュを使っていない

- **主張**: リクエストに `cache_control` が無く、Anthropic ではラウンドごとに履歴全体が正規の入力料金になる。
- **根拠**: `crates/` に `cache_control` は 0 件。実利用の Activity 集計 (`real-usage-aggregates.txt`) で、Opus 5.5 の 28 ラウンドは入力 688,901 トークン、`cache_read` 0、`cache_creation` 0。同じ期間の DeepSeek は入力 28,808 に対し `cache_read` 138,496、OpenAI も `cache_read` が付いている (こちらが何も指定していないので、サーバー側で自動的に効いたと読める)。同じターン内の連続ラウンドだけを数えても、688,901 のうち 500,459 トークン (73%) は直前のラウンドと同じ接頭辞である。
- **成立条件と影響**: Anthropic 形式を使う全員。claude-api リファレンスでは Opus 5.5 の入力は $4/MTok、キャッシュ読み出しは $0.20/MTok (0.05 倍)、5 分キャッシュの書き込みは $5/MTok。長い tool ループほど差が開く。subagent も自分の tool ループを同じ条件で回すので、委任が増えるほど効く。
- **確度**: 確認済み (ログと実装)。削減額は試算で、実 API では未確認。
- **検討した反論**: 互換 endpoint (DeepSeek、Z.ai) がこの項目を拒否する可能性がある。`adaptive_thinking` と同じく `features` で送信を制御すれば済む。承認待ちが 5 分を超えるとキャッシュは切れる。
- **推奨**: トップレベルの `cache_control: {"type": "ephemeral"}` を、組み込み Claude モデルの feature として送る。履歴が追記専用で、要約後の接頭辞も安定しているので、配置を自前で管理する必要はない。あわせてセッションの累積トークンと概算費用を表示する (N13)。

### N5 (P2) サンドボックスをユーザーが広げる口がなく、標準的な開発環境で検証と Git の作業が回らない

- **主張**: 実行方針がコードに固定され、モデルにもユーザーにも拡張の手段がないため、N1 を直しても多くのプロジェクトで「ビルドしてテストしてコミットする」が通らない。
- **根拠と派生する症状** (すべて同じ原因):
  1. crates.io に依存するプロジェクトは `cargo build` も `cargo build --offline` も失敗する (再現 R6)。`~/.cargo` が見えず、ネットワークも無いため。文書の回避策は「プロジェクト内に用意したキャッシュ」だが、子プロセスの環境変数は固定で、モデルが `env CARGO_HOME=… cargo …` を自分で組む必要がある。システムプロンプトも tool の説明もそれを教えない。依存キャッシュをホームに置く Go・Maven・pnpm・uv も同じ形になるはずだが、実行していない (推測)。
  2. PATH のうち `$HOME` 配下でシステムの実行環境に解決されないものは黙って除かれ、同じ名前の次の実体が使われる (`execution/linux.rs:21-35`)。このホストでは `python3` が 3.13.0 (mise) から 3.13.11 (`/nix/store`) に変わった (再現 R6、`real-usage-aggregates.txt`)。`node` は mise の実体が除かれ、`~/.nix-profile` の同じ版に解決されたので差は出なかった。rustup や nvm だけで道具を入れている環境では、コマンドが見つからないか、ディストリビューションの古い版になる。
  3. コマンドは最長 120 秒で、バックグラウンド実行は無い (`constants.rs:11`)。
  4. linked worktree は起動時のプローブで拒否される (再現 R14)。所有者の `socium-wt/*` は 3 つとも該当する。ワークスペースごとに 1 プロセスという制約 (`README.md`) と合わせると、同じリポジトリで並行して作業する手段がない。
  5. エージェントが作った新規ファイルは `git_commit` でコミットできず、`run_command` から `git add` もできない (再現 R7)。identity はリポジトリローカルの設定が必須で、`~/.gitconfig` だけの利用者はコミットが失敗する (再現 R7)。
  6. Ubuntu では bwrap の store path ごとに root で AppArmor プロファイルを入れる必要がある。このホストには bubblewrap 0.12.0 が 3 つの store path にあり、プロファイルは完全一致のパスで 2 つ (`socium-nix-bwrap`、`codex-nix-bwrap`)。flake の更新で hash が変わると、プロファイルを入れ直すまで起動できない。
- **成立条件と影響**: Nix 以外で道具を入れている利用者、Rust・Go・Node のプロジェクト、worktree を使う利用者、新規ファイルを作る作業のほぼすべて。目標 1 (導入の容易さ) と「サンドボックス必須」が正面から衝突している。
- **確度**: 1〜5 は確認済み、6 はホストの設定を確認。このリポジトリの pre-commit (cargo clippy と test) が `git_commit` の中で失敗することと、socium のテストが入れ子の bwrap で動かないことは、1 と `--disable-userns` からの推測で、実行していない。
- **検討した反論**: いずれも `docs/linux-sandbox.md` に制限として書かれており、不具合ではなく設計である。批判の対象は個々の制限ではなく、前提の「この箱の中で日常作業が回る」が、依存キャッシュをホームに置く言語と版管理ツールでは成り立たないこと。
- **推奨**: モデルから書けない場所 (`~/.config/socium/`) の設定で、読み取り専用の追加マウント (`~/.cargo/registry`、`~/.rustup`、mise の installs、`~/go/pkg/mod`)、PATH の追加、時間上限を宣言できるようにする。「モデルは権限を要求できない」原則は変えない。新規ファイルは承認画面に出したうえで `git_commit` が intent-to-add する。worktree は gitdir と commondir を解決して読み取り専用で見せる。AppArmor は glob のプロファイル例を文書に載せる (このホストの `slack-nix` がその形)。取らなかった案: ホスト全体を読み取り可にする (DSH 型。`~/.ssh` なども読めるようになり、socium の強みを捨てる)、ネットワークを許可する (別の脅威)。どこまで広げるかは 8 章の問い 1。

### N6 (P2) undo は呼び出す経路がなく、その履歴は上限に達すると全編集を止める

- **主張**: 編集履歴は変更前後の全文を無期限に溜めるが、undo を実行できるのは広告されていない tool コマンドだけで、履歴が 64 MiB に達するとすべての編集が拒否される。
- **根拠**: undo の入口は text editor の `undo_edit` だけ (`tools/text_editor.rs:92`)。モデルに見せるスキーマの `enum` は `view/create/str_replace/insert` で (`text_editor.rs:36`)、Anthropic の `text_editor_20250728` は `undo_edit` を廃止している (claude-api リファレンス)。TUI に `/undo` は無い (`crates/tui/src/slash_command.rs`)。履歴は 1 件ごとに変更前後の全文を持ち、編集のたびに全体を 2 回書き直す (`tools/edit_history.rs:126-150`)。再現 R2: 履歴を上限まで埋めると、`str_replace` も `apply_patch` も `State exceeds 64 MiB limit; archive the session before continuing` で拒否され、ファイルは変わらない。`/clear` をしても同じ (履歴はセッションではなくワークスペースに属する)。文言の「archive the session」に対応する操作は無い。release ビルドで、31 MB の履歴がある状態の 4 バイトの編集は 0.3〜0.4 秒 (空のとき 0.1 秒未満)。
- **成立条件と影響**: 実利用の履歴は 9 件で 88 KB (1 件約 10 KB)。この比率なら約 6,800 件、大きいファイルを編集するワークスペースなら 1,000〜2,000 件で届く。届いた後は `.socium/edit_history.json` を手で退避するまで、そのワークスペースでは編集できない。
- **確度**: 上限での拒否は確認済み。undo の到達性は高 (コードとスキーマから明白。モデルがスキーマ外の `undo_edit` を送れば動く)。
- **検討した反論**: 履歴は undo のためだけでなく「変更前に記録を永続化する」ためにもある。ただし復旧手順 (`docs/daily-reliability.md`) が使うのは未完了の 1 件だけで、完了済みの記録を溜め続ける理由にはならない。
- **推奨**: TUI に `/undo` (直近の編集、またはパス指定) を追加し、README の「undo recorded edits」を実際に使える機能にする。完了済みの記録は件数とサイズで古いものから捨てる。取らなかった案: 履歴をセッションごとに分ける (ADR 7 が退けている。同じファイルへの取り消しが会話をまたいで壊れる)。

### N7 (P2) 要約が元の依頼の後半を落とす

- **主張**: 要約モデルに渡す入力は 1 メッセージ 600 文字、tool 入力 120 文字で切られ、出力は 1,500 トークンなので、長い依頼文の制約は要約に入らない。
- **根拠**: `agent/compaction.rs:101-169`、`constants.rs:35-36`。再現 R5: 約 1,400 文字の依頼の末尾に置いた要件は、要約リクエストにも、要約後のリクエストにも現れない。LLM の要約が失敗したときの代替は直近 24 行・各 240 文字の機械的な列挙で (`context.rs:438-473`)、失敗の理由は `eprintln!` にしか出ない (`lib.rs:1201`)。画面には「deterministic fallback」としか残らない。
- **成立条件と影響**: 仕様やログを貼った長い依頼の後で圧縮が走ったとき。N3 のとおり、既定では実際の窓の約 11% で走る。モデルは最初の 600 文字だけを「依頼」として続きを進める。
- **確度**: 確認済み。
- **検討した反論**: `741f023` は要約の追記で新しい修正が消える問題を直したが、入力側の切り詰めは対象外だった。
- **推奨**: ユーザーの発言は全文 (上限つき) で要約に渡し、最初の依頼は要約とは別にそのまま保持する。切り詰めは文字数ではなくトークン予算で行う。失敗の理由は Activity の Warning に出す。

### N8 (P2) 脅威モデルが明文化されておらず、承認なしの編集がホストで実行されるファイルに届く

- **主張**: 暗黙の脅威モデルは「モデルの出力は信頼しないが、ワークスペースへの書き込みは信頼する」で、ワークスペースの内容をホスト権限で自動実行する環境ではサンドボックスの外へ出る経路になる。
- **根拠**: 再現 R16。承認経路なしで、`build.rs`・`.envrc`・`.github/workflows/ci.yaml` の編集が成功し、`.envrc` の内容も読める。`run_command` だけが拒否される。`SECURITY.md:10-12` は「要求されたコマンドと diff を確認せよ」と書くが、編集の diff を見せる画面は無い (N13)。`.envrc.example` は API キーをワークスペース内の `.envrc` に置く例で、`README.md` の「Keep API keys outside source files」と `SECURITY.md:30` の「Do not put credentials in the workspace」と食い違う。読んだ内容はプロバイダへ送られ、`.socium/` に平文で残る。
- **成立条件と影響**: リポジトリ内のファイルや tool 出力に仕込まれた指示にモデルが従った場合、またはモデルの誤り。rust-analyzer (build script と proc macro を自動実行)、direnv、pre-commit、`nix develop` の shellHook、CI を使う環境で、次にホストがその内容を実行する。通常利用で必ず起きるものではないので P2。
- **確度**: 承認なしで編集できることは確認済み。ホスト側で実行されるかは環境に依存し、実行していない。
- **検討した反論**: 自動編集を許すエージェントに共通の性質で、socium だけの欠陥ではない。ただし socium は「サンドボックス必須・フォールバックなし」を安全性の中心に据えているので、境界がどこにあるかを書かないと保証を誤解させる。
- **推奨**: 脅威モデルを 1 ページで明文化する (守るもの、守らないもの、信頼する入力)。ホストが自動実行するパス (`build.rs`、`.envrc`、`flake.nix`、`.cargo/config.toml`、`.github/workflows/`、`package.json` の scripts、`Makefile`、`.pre-commit-config.yaml`) への編集は、承認するか、少なくとも目立つ通知にする。`.env*` と `.envrc` は既定で読み取りを拒否し、ユーザー設定で外せるようにする。取らなかった案: すべての編集を承認制にする (承認疲れで形骸化する)。

### N9 (P2) 承認が毎回・120 秒・既定で許可という組み合わせになっている

- **主張**: すべてのコマンドに 1 回限りの承認が要り、120 秒で自動的に拒否され、ダイアログの既定の選択は「許可」なので、席を外す使い方ができず、急いだ Enter は許可になる。
- **根拠**: `control.rs:46` (120 秒)、`crates/tui/src/approval.rs:71,93-96` (初期選択が `AllowOnce` で Enter が確定)。実利用の承認 8 件は、待ち時間が 3〜62 秒の許可 6 件、拒否 1 件、120,012 ms での時間切れ 1 件 (`real-usage-aggregates.txt`)。継続的な許可は無い。
- **成立条件と影響**: テストを何度も回す作業と、長いタスクを任せて離れる使い方。時間切れはモデルには「拒否」として返り、モデルは検証を諦めるか別の手段を探す。
- **確度**: 高 (コードと実ログ)。
- **検討した反論**: サンドボックスがコマンドの効果をワークスペース内に閉じているので、承認が追加で守っているものは小さい。同じ書き込みは承認なしの編集 tool でできる。残る違いは、コマンドによる変更は edit history に記録されず、取り消せないこと。承認はその破壊的な変更からの保護として意味を持つ。
- **推奨**: 8 章の問い 2 の判断による。最小限の変更は、既定の選択を「拒否」にする (または表示から一定時間は Enter を無視する) ことと、時間切れを拒否と区別してモデルに伝えること。

### N10 (P2) モデル切り替え時の thinking の扱いが 3 通りあり、ADR 6 の前提が現行仕様と合わない

- **主張**: 履歴に生成元 (形式とモデル) の記録がないため、thinking の扱いが経路ごとに違い、`/model` は生履歴から thinking を恒久的に消す。
- **根拠**: (1) `/model` とセッション再開時のモデル不一致は、生履歴の thinking を破壊的に除去して保存する (`lib.rs:680,906`、`agent/types.rs:117-133`)。(2) 要約後は送信時だけ除去し、生履歴は残す (`context.rs:253-263`)。(3) `openai-responses` は adapter 側で署名の形を見て捨てる (`providers/openai_responses/mod.rs:203-207,261-265`)。(4) `MODEL` 環境変数や registry の既定で決まったモデルは保存されないので、次の起動で別のモデルになっても何も除去されない。再現 R4: Opus で作った履歴を `MODEL=ds-flash` で開くと、DeepSeek 宛てのリクエストに Anthropic の署名と `redacted_thinking` がそのまま入る。ADR 6 は決定要因に「別のモデルの thinking 署名を送らない」を挙げている。
- **外部仕様との関係**: claude-api リファレンスは、Anthropic のモデル間では「thinking ブロックはそのまま送り返す。読めないブロックは API が課金せずに落とす。自分で除去しない」としている。Opus 5.5 は Sonnet 5.5 のブロックを読めるので、`/model sonnet` → `/model opus` での除去は不要な損失になる。ADR 6 の「署名はそれを出したモデルにしか通らない」という前提は、Anthropic どうしでは当てはまらない。
- **成立条件と影響**: Anthropic のモデル間を行き来する利用者は、切り替えのたびに推論の連続性を失い、生履歴からも戻せない。Anthropic 以外の互換 endpoint が他モデルの thinking をどう扱うかは未確認 (推測: 拒否する実装がありうる)。
- **確度**: 経路の違いと再現 R4 は確認済み。互換 endpoint の挙動は未確認。
- **推奨**: assistant メッセージに生成元 (provider 名とモデル ID) を記録し、生履歴は変えずに、送信時に adapter が判断する。Anthropic 本家へは全部送り、それ以外へは同じ生成元のものだけ送る。pi も送信時の変換で他モデルの thinking を落としている ([transform-messages.ts:128-136](https://github.com/earendil-works/pi/blob/c1d4b0ac3e35dc4eb1c5d00d90bf54003cf517c0/packages/ai/src/api/transform-messages.ts#L128-L136))。SOC-014 の「provider 非依存の契約」の最初の具体的な項目になる。

### N11 (P2) stream の厳格さが将来のイベントと回復可能な失敗まで巻き込む

- **主張**: 未知のイベント種別、出力上限、不正な tool 引数のいずれでも応答全体が失敗し、自動の再試行も無い。
- **根拠**: 再現 R9。知らない `type` のイベントが 1 つ混じると `unknown variant` で失敗する (`providers/anthropic/stream.rs:155`)。公式の文書は「新しいイベント種別が追加されることがあり、未知のイベントは無視できるようにせよ」と書いている (2 章の出典)。`max_tokens` で止まった応答は本文ごと捨てる (`stream.rs:276-282`)。実利用のログにも `max_tokens` による失敗が 1 件ある。eager streaming で壊れた tool 引数の JSON はターンの失敗になる (`stream.rs:312`)。429・529・接続断の再試行は無い。
- **成立条件と影響**: Anthropic か互換 endpoint がイベントを追加した日に、全リクエストが失敗する。長い出力は N2 と合わせて二重に失われる。
- **確度**: 確認済み (未知イベント)、高 (それ以外)。
- **検討した反論**: 厳格さは F7 の修正の要で、tool を解放する条件は緩めてはいけない。緩めるのは「知らない種別を読み飛ばす」ことだけで、ブロックの順序や停止理由の検証は残せる。
- **推奨**: 未知のイベントと未知の delta は読み飛ばす (raw view には残す)。`max_tokens` で tool 呼び出しを含まない応答は、打ち切りの印を付けて履歴に残す。壊れた tool 引数は、その tool のエラー結果としてモデルに返す。再試行は副作用の前 (モデルへのリクエスト) に限定して導入する。

### N12 (P2) tool の契約とプロンプトが、モデルに必要な情報を渡していない

同じ原因 (tool 出力が素の文字列で、モデル向けの契約として設計されていない) の症状をまとめる。

| 症状 | 根拠 | 影響 |
|---|---|---|
| プロンプトは `path:line` の引用を求めるが、`read_file` も `view` も行番号を返さない | 再現 R10、`constants.rs:39`、`tools/read_file.rs:73`、`tools/text_editor.rs:124` | モデルが行を数えることになり、引用と `insert` の行指定が外れる |
| 主エージェントのシステムプロンプトが 3 文で、サンドボックスの制約、承認、検証の進め方、プロジェクトの指示ファイルを伝えない | `constants.rs:39`。`AGENTS.md` などの読込は 0 件。一方、`worker` subagent のプロンプトは承認の扱い、既存の変更の保護、検証の報告を指示している (`constants.rs` の `SUBAGENT_WORKER_SYSTEM_PROMPT`) | N5 の回避策をモデルが知る手段がない。子のほうが親より多くを教えられている。SOC-022 |
| コマンド出力は先頭 20,000 文字 | `tools/run_command.rs:292-306` | ビルドやテストの失敗は末尾に出る。pi は末尾を残し全文をファイルへ退避 ([truncate.ts:168](https://github.com/earendil-works/pi/blob/c1d4b0ac3e35dc4eb1c5d00d90bf54003cf517c0/packages/coding-agent/src/core/tools/truncate.ts#L168)) |
| `read_file` は範囲指定なしで最大 200 KB | `tools/read_file.rs:61-74` | N3 の主因。範囲指定は `view` にだけある |
| dry-run が変更前後の全文を返す | 再現 R13: 20,007 バイトのファイルの 1 行変更で 48,082 バイト | F8 の修正の副作用。確認のたびにファイル 2 つ分の文脈を使う |
| `Add File` で作ったファイルに末尾の改行が付かない | 再現 R13、`tools/apply_patch.rs:294`、既存テスト `apply_patch_creates_new_file` がこの挙動を固定している | 作るファイルすべてが `\ No newline at end of file` になる |
| `*** End of File` と空行の文脈行を拒否する | 再現 R13、`apply_patch.rs:330,357` | 同じ形式を学習したモデルが普通に出す書き方。エラーは返るので再試行はできる |
| 再帰の `list_files` が `.git` と ignore 対象を含み、1,000 件で任意に打ち切る | 再現 R8: `target/` が 999 件を占め `src/lib.rs` が出ない。`target/` を消すと 31 行中 26 行が `.git/` | Rust のリポジトリの根で使うと役に立たない |
| 検索結果の 1 行に長さの上限がない | `tools/ripgrep.rs:179-189` | minify されたファイルで 1 回の結果が 256 KiB に届く |

- **成立条件と影響**: すべてのセッション。個々は小さいが、ラウンド数と文脈の消費を押し上げ、N3 の上限に早く届かせる。
- **確度**: 確認済み (再現のあるもの)、高 (それ以外)。
- **検討した反論**: tool の粒度や出力の形は、モデルで測らないと優劣が決まらないものが多い (N14)。ここに挙げたのは、測らなくても片方が劣ると言えるものに限った。
- **推奨**: 読み取り系は行番号つきで返し、`read_file` に範囲を付ける (または `view` に一本化する)。コマンド出力は末尾優先にする。dry-run は変更箇所の前後だけを返す。`Add File` は末尾に改行を付ける。再帰の一覧は `rg --files` に寄せる。システムプロンプトには、サンドボックスの制約と検証の進め方を短く加える。順序は N14 の評価基盤ができてから測って決めるのが望ましいが、行番号と末尾優先は測るまでもない。

### N13 (P2) TUI: 複数行を貼り付けられず、何が変わったかも見えない

- **主張**: 入力欄は 1 行で bracketed paste を有効にしていないので、複数行の貼り付けは最初の行だけが送信され、残りは捨てられる。
- **根拠**: `crates/tui/src/` に `Paste` の処理は 0 件、キー入力は `Event::Key` だけ (`app.rs:507`)、実行中の文字入力は捨てる (`app.rs:590-634`)。tmux 上の実端末で 3 行を貼り付けると、1 行目だけが送信され、中断後の入力欄には何も残らなかった (`tui-paste.txt`)。
- **派生する症状**:
  - 実行中は次の指示を打てない (SOC-035)。
  - アイドル時の Esc は Normal モードへの切り替えで、そこで `q` を押すと確認なしに終了する (`app.rs:565`)。Esc のあと「q」で始まる文を打つと終了する。
  - 編集 tool の結果は会話欄と Activity 欄に `Path: …` の 1 行で出るだけで、差分は見えない (`activity.rs` の `summarize_tool_result`)。Activity の項目を展開すれば入力の値 (`old_str`・`new_str` や patch の本文) は読めるが、差分の形ではない。「エージェントが何を変えたか」をまとめて見るには、別の端末で `git diff` を打つことになる。
  - 累積トークンと費用の表示が無い。残量の表示は出力枠を引いた値なので、空の会話でも「68% context left」から始まる (`tui-paste.txt`)。
  - `eprintln!` が 3 か所にあり (`app.rs:362`、`lib.rs:1050,1201`)、TUI が描画している端末にそのまま書かれる (表示の乱れ方は実機で確かめていない)。
- **成立条件と影響**: エラーログや仕様を貼って依頼する場面。1 行目だけの依頼でモデルが動き出し、利用者は中断して打ち直すことになる。
- **確度**: 貼り付けは確認済み、それ以外は高。
- **検討した反論**: 端末によっては貼り付けを 1 行にまとめる設定があるが、既定ではない。差分は `git_commit` の承認画面には出るので、コミットまで進めば確認はできる。
- **推奨**: bracketed paste を有効にして `Event::Paste` を入力欄に挿入し、複数行の入力を持てるようにする。編集の結果には unified diff の要約 (追加・削除の行数と先頭数行) を付ける。Normal モードの `q` は廃止するか確認を挟む。

### N14 (P2) 改善を測る手段がなく、テストが「実際の道具が動くか」を見ていない

- **主張**: headless の実行経路と評価が無いので、プロンプトや tool の変更が良くなったかを判定できず、既存のテストは N1・N2・N3 のどれも検出しなかった。
- **根拠**: CLI の引数は `-c`・`-r`・`-h` だけ (`crates/tui/src/cli.rs`)。既存テストの 1 件 (`network_and_kernel_escape_interfaces_are_denied`) は一時ディレクトリのパスが長いと Unix socket の長さ制限で失敗する (`baseline-tests-long-tmpdir.log`。既定の TMPDIR では成功)。サンドボックスのテストが動かすのは `sh`・`sed`・`env`・`git`・`setsid` (`execution/tests.rs`)。HTTP のテストは即時に返る応答だけ。予算のテストは小さい窓での単体。実 API のテストは 6 件が手動。TUI は pty 上で動かしていない。Anthropic 互換 endpoint (DeepSeek、Z.ai) は契約テストが無く、実利用のログでは 17 件の失敗のうち 13 件が Z.ai の認証と残高のエラーだった。
- **成立条件と影響**: 今後の変更すべて。SOC-022・033 (プロンプトと tool 構成) は測れないまま進めることになる。
- **確度**: 高。
- **検討した反論**: テストが弱いわけではない。保存境界の故障注入、サンドボックスの脱出経路、stream の契約は丁寧に押さえている。足りないのは「守る性質」ではなく「使える性質」のテストである。
- **推奨**: 順に、(1) サンドボックス内で実際のツールチェーンを動かす受け入れテスト (N1 の再発防止)、(2) 遅い・長い・壊れたストリームを返すローカルサーバーでの provider テスト、(3) `ActivityEvent` をそのまま流す headless の JSONL 出力 (ADR 2 が想定済み)、(4) 20 件程度の固定タスクでの評価。pi は `packages/evals` に Docker で隔離した評価を持っている (README の記述。実行していない)。

### N15 (P3) 構造と状態の持ち方

- `AgentService` がモデル選択、セッション、圧縮、ターンの進行、保存を 1 つの型で持つ (`crates/core/src/lib.rs` は 2,263 行で、うち実装は約 1,250 行)。イベントは型のない `serde_json::Value` の列で (`lib.rs:236`)、`transcript.rs` が文字列キーで読み戻している。
- 進捗の通知が `AgentProgress` と `ActivityEvent` の二系統のまま。ADR 2 は移行後に重複を削るとしていた。
- 環境変数の読み取りが注入された lookup と `env::var` の直読みに分かれている (`context.rs:70`、`compaction.rs:16`)。
- チェックポイントのたびに会話全体と `meta.json` を書き直す。実利用の `conversation.json` は、メッセージ 80.5 KB に対してイベントが 44.5 KB (うち 21 KB は tool 入力の複製)。
- 会話が 64 MiB に達すると送信は失敗し、その会話は続けられない (再現 R11)。`/clear` で新しいセッションには移れる。
- `flake.nix` は darwin を `systems` に挙げているが、実行は未対応。バイナリ名は `socium-tui` で、リリース物は無い。
- **確度**: 高 (コードとファイルサイズから)。会話の上限だけ再現した。
- **推奨**: 急がない。N3 の修正で投影の処理が増える前に、`ContextAssembler` に当たる部分を `lib.rs` から切り出すと変更しやすい。イベントは型を付ける。

### 反証で落とした、または弱めた指摘

| 当初の仮説 | 結果 |
|---|---|
| 巨大な tool 結果でセッションが恒久的に詰まる | 否定。7 回の失敗の後に復帰する。N3 の記述を修正した |
| キー入力ごとの文脈計測が長い履歴で入力を遅らせる | 取り下げ。release ビルドで、要約済みの 32 MiB の履歴でも数ミリ秒だった (`timings-release.log`) |
| 別の Anthropic モデルへ thinking を送ると 400 になる | 弱めた。リファレンスでは API が黙って落とす。N10 は「不要な損失と経路の不一致」として書いた |
| edit history の肥大で編集が実用にならないほど遅くなる | 弱めた。release ビルドでは 31 MB で 0.3〜0.4 秒。問題は上限到達後の拒否 |
| 再試行が無いことが実利用の失敗の主因 | 否定。実ログの失敗 17 件に再試行で救えるものは無かった (認証、残高、不正な組み合わせ、出力上限) |

## 6. pi / DSH / その他との比較

**前回の固定点からの変化**

- pi (`682c7b93` → `c1d4b0ac`、176 コミット。docs 66、fix 60、feat 43): コミット件名で確認した範囲では、中断時にすべての tool 呼び出しへ結果を返す修正、末尾が 1 行で長い出力を保持する修正、`Connection closed.` の再試行、signal 終了を失敗として報告する修正、会話の場所ごとの MCP。socium が既に持つ性質 (結果の対の保持、signal の扱い) を後から直しているものが含まれる。差分の本文は読んでいない。
- DSH (`fcc0ad46` → `39163dd3`、1,371 コミット。fix 306、test 174、feat 169): sandbox・approval・permission・compaction・worktree に触れる件名が 66 件。worktree サービス (コマンドの既定の期限 5 分)、要約を「最後に答えたモデル」で行う修正、承認拒否のエラーコード化など。9 日でこの量が動いており、前回の評価「合成と移行の複雑さ」は今も当てはまる。

**差分ごとの判断**

| 観点 | pi | DSH | socium | 判断 |
|---|---|---|---|---|
| 実行の隔離 | なし。project trust の確認はある (`project-trust.ts`) | 書き込みの閉じ込め。Landlock は `/` を読み取り可、Seatbelt は既定許可で書き込みだけ拒否 ([profiles.ts](https://github.com/deepseek-ai/deepseek-harness/blob/39163dd30f703a79079e2d2446545cff3c453738/packages/sandbox/sandbox-local/src/profiles.ts) で確認)。追加の書き込み先を設定できる (README) | 読み取りもネットワークも環境変数も遮断 | **socium が優れる**。ただし広げる口がない (N5)。DSH の「ユーザーが宣言する追加ルート」は採用、ホスト全体の読み取り可は不採用 |
| socketpair | — | — | 全拒否 | Codex は `AF_UNIX` を許可 (実装で確認)。別解として型で絞る (N1) |
| 承認 | なし | 名前つきのプリセットがサンドボックスと承認を一緒に切り替える (README) | 毎回、1 回限り | プリセットの数を増やすのは不採用。「サンドボックス内は自動、外へ出る操作だけ承認」の 2 値は検討に値する (8 章) |
| 文脈の超過 | 閾値と overflow の 2 経路で自動圧縮し、overflow は 1 回だけ回復を試みる (`agent-session.ts:223,439`)。保持はトークン数で決める (`compaction.ts:128-129`) | tool 結果を先頭と末尾に縮める pruner。モデルを呼ばず、原本はログに残る (README) | ターンの失敗 | DSH の pruner と pi のループ内圧縮を採用 (N3)。pi のツリー状セッションは不採用 |
| tool 出力 | 2,000 行または 50 KB、bash は末尾優先で全文をファイルへ、read は `offset`/`limit` (`truncate.ts:11-13`、`read.ts:16-17`) | — | 先頭 20,000 文字、read は 200 KB 一括 | 採用 (N12)。上限値は socium で測って決める |
| モデル切り替え時の thinking | 送信時の変換で落とす (`transform-messages.ts:128-136`) | — | 生履歴から消す | 採用 (N10) |
| 貼り付け | bracketed paste (`tui/src/terminal.ts:219`) | — | なし | 採用 (N13) |
| 指示ファイル | `AGENTS.md`・`CLAUDE.md` を読む (`resource-loader.ts:213`) | あり (前回確認) | なし | 採用。まず root の `AGENTS.md` だけ、サイズ上限つき |
| 費用 | 累積の cost (`usage-totals.ts`) | — | なし | 採用 (N13)。価格表を持つかは 8 章 |
| headless・評価 | print・json・rpc の各モードと `packages/evals` | SDK・ACP・benchmarks | なし | headless の JSONL だけ採用 (N14)。RPC と SDK は不採用 |
| 再試行 | 自動 (`agent-session.ts:235`) | — | なし | 副作用の前に限って採用 (N11) |
| 規模 | `coding-agent/src` 約 75,000 行、`agent`+`ai`+`tui` 約 31,000 行 | `packages/` 直下に 53 の群 | 約 29,400 行 | **socium が優れる**。拡張機構は不採用のまま |

**socium のほうが優れている点** (上の表の再掲を除く): 実行済み操作の逐次保存と「結果不明は再実行しない」の規約、commit の範囲限定と承認中の変化の検出、patch の事前検証と行末・mode の保持、要約後の thinking の扱い。pi が今回の期間に修正した「中断時の tool 結果の対」(件名による) は、socium では `repair_incomplete_tools` と `validate_protocol` で守られている。

## 7. ドキュメントと実装の矛盾

**文書が古い、または互いに食い違う**

| 文書 | 記述 | 実装・他の文書 |
|---|---|---|
| `docs/daily-reliability.md:44` | move と undo は実行ビットを戻さない (SOC-040) | `336270d` で実装済み。`README.md:131-133` は「保持する」 |
| `CONTRIBUTING.md:3` | 実装済みの provider は Anthropic だけ | `openai-chat` と `openai-responses` がある |
| `SECURITY.md:29` | 読んだ内容は Anthropic に送られうる | 登録した任意の接続先に送られる |
| `docs/todo.md:19-20` | OpenAI 互換とローカルモデルは未実装 | `fd143b8` で実装済み |
| `docs/improvement-plan.md:20` | 設定ファイルは未着手、現在は環境変数 | `models.toml` がある |
| `README.md` 「undo recorded edits」 | — | 利用者が呼ぶ経路がない (N6) |
| `SECURITY.md:11-12` 「Inspect requested commands and diffs」 | — | 編集の diff を見せる画面がない (N13) |
| `README.md` と `SECURITY.md:30` 「キーをワークスペースに置くな」 | — | `.envrc.example` はワークスペースの `.envrc` に置く例 |
| `README.md` 「Web search is planned; see ADR 6」 | — | ADR 6 に Web 検索の記述は無い (SOC-037 が該当) |
| `docs/linux-sandbox.md:81-84` | socketpair を使う IPC は未対応 | cargo・rustc・asyncio・Node が該当することが書かれていない (N1) |
| ADR 4 | 保存先は `.socium/activity.jsonl`、`/clear` でも回転する | ADR 7 でセッションごとになり、`/clear` は回転しない。ADR 4 に 7 へのリンクが無い |
| ADR 6 | 選んだ alias を `.socium/conversation.json` に保存。「署名はそれを出したモデルにしか通らない」 | 保存先はセッションの snapshot。新しいセッションは直前のセッションの選択を引き継ぐ (ADR 7)。後段は N10 |
| ADR 2 | `AgentProgress` は移行後に重複を削る | 二系統のまま |
| ADR 8、9、10 | `status: proposed` | 3 つとも実装されて `main` に入り、README にも載っている。ADR 7 までは `accepted` |
| `docs/design-agent-activity-stream.md` | 状態「決定済み・実装中」 | Phase 0〜4 は実装済み |
| `docs/tickets/README.md` | 現在の状態は GitHub を正とする | GitHub で open の SOC-016・028・040 は実装済み。チケット本文の「状態: 未着手」も同じ |
| `flake.nix` | `systems` に darwin | 実行は未対応 (`README.md`) |

**実装にあるのに文書化されていない決定**

- 環境変数 `CONTEXT_WINDOW_TOKENS` (`agent/context.rs:70`。エラー文が設定を促すのに README に無い)、`COMPACTION_TOKEN_THRESHOLD` (`agent/compaction.rs:16`)、`ANTHROPIC_VERSION` (`providers/anthropic/mod.rs:41`)。
- モデルが `max_output_tokens` を持つと `MAX_TOKENS` は警告なしに無視される (`lib.rs:554-558`)。
- ADR の無い決定: サンドボックス必須とフォールバックなし (`docs/linux-sandbox.md` にはあるが、退けた選択肢の記録が無い)、承認が毎回・120 秒、1 ターン 20 ラウンド、コマンド 120 秒、HTTP 90 秒、組み込みモデルの窓 20 万、圧縮の閾値 80% と直近 16 メッセージの保持、状態の上限 64 MiB、追記ログではなくスナップショットを選んだこと、システムプロンプトを最小にしていること。N1・N2・N3 はどれも、理由が記録されていない定数から来ている。

## 8. 所有者が決めるべき問い

技術的に一意に決まらない判断。選択肢と帰結を添える。

**問い 1: サンドボックスをどこまで、誰が広げられるようにするか** (N5)

| 選択肢 | 帰結 |
|---|---|
| A. 現状のまま固定 | 保証は最も単純。Nix で道具を入れ、依存をプロジェクト内に置ける利用者だけが検証まで回せる。目標 1 は「Nix 利用者にとって容易」に読み替える |
| B. ユーザー設定で読み取り専用マウントと PATH と時間上限を宣言 (推奨) | モデルは権限を要求できない原則を保ったまま、Rust・Go・Node の標準的な構成で動く。設定を誤ると `~/.ssh` を見せることもできるので、既定は空、起動時に有効な追加分を表示する |
| C. 承認ダイアログでその場の権限拡大を許す | 柔軟だが、モデルが出した要求を人が評価する形に戻り、F1 で閉じた面を開ける |

**問い 2: 承認は何を守るためのものか** (N9)

| 選択肢 | 帰結 |
|---|---|
| A. 毎回承認 (現状) | 取り消せない変更を必ず人が見る。離席中は 120 秒ごとに拒否され、長いタスクは任せられない |
| B. サンドボックス内のコマンドは自動、`git_commit` だけ承認 | 離席して任せられる。コマンドによる変更は edit history に入らないので、実行前にワークスペースのスナップショット (例: 一時的な commit や stash 相当) を取る仕組みが前提になる |
| C. セッション内で「このコマンドは以後許可」を持つ | A と B の中間。許可の単位 (argv 完全一致か、先頭の語か) の設計が要る。先頭の語にすると F3 の「名前で権限を決める」に近づく |

**問い 3: 既定の入力予算をいくつにするか** (N3、N4)

窓を実値 (リファレンスでは 100 万) にすると N3 はほぼ消えるが、キャッシュ無しではラウンドごとの費用が履歴に比例して増える。N4 を先に入れるなら大きい予算が妥当で、入れないなら 20〜30 万で圧縮するほうが費用は読める。「窓」と「圧縮を始める点」を別の設定にし、後者を利用者に見せるのが扱いやすい。

**問い 4: 誰が、どの場面で socium を選ぶのか**

現状の強みは「ホームもネットワークも見せない隔離」と「何をしたかの記録」で、弱みは導入と検証ループである。

| 位置づけ | 優先すべきこと |
|---|---|
| A. 信頼しきれないリポジトリやモデルを安全に動かす道具 | N1、N8、脅威モデルの明文化。隔離は広げない |
| B. 所有者自身の日常の開発道具 | N1、N2、N3、N5、N13。worktree と並行実行 |
| C. ハーネスの実装を学び、試す土台 | N14 (headless と評価)、N15。小ささの維持 |

B と A は問い 1 で衝突する。レビュー中に入った subagent は B の方向の投資で、同期実行と読み取り専用の並行に絞った設計は、1 ワークスペース 1 プロセスのロックや一度に 1 件の承認と衝突しない。次に来るはずの「編集する子の並行」と「バックグラウンド実行」は、そのロックと、チェックポイントごとに全体を書き直す保存 (N15) の両方に当たる。そこへ進む前に N1〜N3 を直すほうが、子の数だけ同じ失敗が増えるのを避けられる。

**問い 5: Anthropic 互換 endpoint をどこまで保証するか** (N10、N11、N14)

実利用の失敗の大半はここで起きている。「登録できるが保証しない」と明記するか、DeepSeek と Z.ai について契約テスト (記録した応答の再生) を持つかで、stream の厳格さと thinking の扱いの設計が変わる。

**問い 6: 費用の表示に価格表を持つか**

トークンの累積は価格表なしで出せる。金額を出すには `models.toml` に単価を書かせるか、組み込みの表を保守する必要がある。前者は利用者の手間、後者は古くなる表を抱える。

## 9. 付録

**証拠ファイル** (`docs/review-evidence/2026-11-14/`)

| ファイル | 内容 |
|---|---|
| `run_repros.sh` | 一時ディレクトリに指定コミットを展開し、以下のログを作る。リポジトリには書き込まない |
| `review_repros.rs` | 再現テスト 15 件 (R1〜R14、R16。R12 は取り下げた仮説の計測、R15 は欠番) |
| `baseline-tests.log`、`baseline-tests-c3b8790.log`、`baseline-tests-ddb44df.log` | 既存テストの結果 (`2d4ac91`、中間の `c3b8790`、開始時の `ddb44df`) |
| `baseline-tests-long-tmpdir.log` | 長い TMPDIR で実行したときの `2d4ac91` の結果 (1 件失敗。N14) |
| `repros.log` | 再現テストの出力 (R1 と R12 を除く) |
| `repro-r1-http-timeout.log` | 90 秒の期限 |
| `repro-r6-socketpair-allowed.log`、`control-socketpair.diff` | 対照実験とその差分 (1 行) |
| `timings-release.log` | release ビルドでの R2・R12 の計測 |
| `real-usage-aggregates.txt` | 実利用の `.socium/` とホスト設定の集計 (数値と種別のみ) |
| `tui-paste.txt`、`tui_paste_check.sh`、`stall_server.py`、`tui-models.toml` | tmux での貼り付けの画面と、その手順 |
| `external-commits.txt` | 外部リポジトリのコミットと確認したファイル |
| `run-info.txt` | `run_repros.sh` が対象にしたコミット |

**再実行**

```sh
docs/review-evidence/2026-11-14/run_repros.sh            # 2d4ac91 に対して、約 6 分
docs/review-evidence/2026-11-14/run_repros.sh <commit>   # 別のコミットに対して
```

再現テストは「指摘した挙動があると成功する」観察で、修正後は失敗する。その時点で期待を反転し、通常の回帰テストへ移す。R は再現テストの番号、N は本文の指摘の番号で、対応は次のとおり。ログの各行は R 番号で始まる。

| 再現 (`review_repros.rs`) | 本文 |
|---|---|
| R1 `review_r1_…` | N2 |
| R2 `review_r2_…` | N6 |
| R3 `review_r3_…` | N3 |
| R4 `review_r4_…` | N10 |
| R5 `review_r5_…` | N7 |
| R6 `review_r6_…` | N1、N5 |
| R7 `review_r7_…` | N5 |
| R8、R10、R13 `review_r8_…` ほか | N12 |
| R9 `review_r9_…` | N11 |
| R11 `review_r11_…` | N15 |
| R12 `review_r12_…` | 取り下げた仮説 |
| R14 `review_r14_…` | N5 |
| R16 `review_r16_…` | N8 |
