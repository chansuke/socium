---
number: 8
title: agentツールで独立した文脈のサブエージェントに作業を委ねる
status: accepted
date: 2026-11-14
---

# agentツールで独立した文脈のサブエージェントに作業を委ねる

## Context and Problem Statement

socium のエージェントは、一つの会話の文脈ですべての作業をする。広い範囲の検索や、本題から切り離せる調査をすると、途中で読んだファイルや検索結果がすべて会話に残る。文脈がすぐに膨らみ、compaction が早く起き、本題に関係の薄い tool 結果が以降の request に毎回送られる。

Claude Code と Codex CLI は、子のエージェントに作業を委ねる仕組みを持つ。

* Claude Code は `Agent`（旧 `Task`）tool を持つ。親のモデルが `description`、`prompt`、`subagent_type` を渡すと、子は空の文脈で自分の tool loop を回し、最後の応答だけが tool 結果として親に返る。子は `Agent` を使えない。種類は組み込み（汎用、読み取り専用の Explore など）と、利用者が `.claude/agents/*.md` に書く定義からなる。
* Codex CLI は `spawn_agent` で子を起動し、`agent_id` を即座に返す。親は作業を続け、`wait`（V2 では `wait_agent`）などの管理用の tool で子の完了を待つか、追加の指示を送る。役割は設定の `[agents]` に書き、`spawn_agent` の `agent_type` で選ぶ。子はさらに子を起動でき、V1 は `agent_max_depth` で深さを制限する。

socium で子のエージェントを使えるようにしたい。

## Decision Drivers

* 子の途中経過を親の文脈に入れず、結果だけを返す。
* 人の承認、OS sandbox、取り消しといった既存の安全の仕組みを、子も迂回できない。
* 子が何をしたかを、利用者が Activity 欄で親の作業と区別して追える。
* 今の tool loop、永続化、TUI を大きく作り直さずに入る。

## Considered Options

* 同期の `agent` tool（Claude Code の型）
* 非同期の子エージェントと管理用の tool 群（Codex CLI の型）
* 同期の `agent` tool に、利用者が定義する種類を最初から加える

## Decision Outcome

Chosen option: 「同期の `agent` tool」。親の tool loop の中で子の tool loop を最後まで回し、子の最後の応答を tool 結果にする。今の tool loop は tool を一つずつ順に実行し、承認も一度に一件だけ待つので、子を並行に動かすと承認の取り違えや、同じファイルへの同時の編集を新たに防ぐ必要が出る。同期なら、その問題は起きない。

### tool

親のモデルには `agent` tool を渡す。入力は `description`（利用者に見せる短い名前）、`prompt`（子への指示のすべて）、`agent_type` の三つで、すべて必須。

* `explorer`: `read_file`、`list_files`、`search_text`、`find_symbol`、`git_branch`、`git_status`、`git_diff`、`git_log` だけを持つ。ファイルを変えず、コマンドも実行しない。
* `worker`: `agent` 以外のすべての tool を持つ。`run_command` と `git_commit` は、親と同じく利用者の承認を一回ずつ要する。

種類を必須にしたのは、親のモデルに権限の大きさを毎回選ばせるため。

### 実行

* 子は親と同じモデル、provider、workspace、`TaskControl`（取り消しと承認）を使う。
* 子の文脈は、種類ごとの system prompt と、`prompt` を本文とする一つの user message だけから始まる。親の会話は見えない。
* tool loop の上限は親と同じ 20 round。
* tool loop は、モデルに渡した tool 以外の呼び出しを `Unsupported tool` として拒否する。子がモデルの判断で `apply_patch` や `agent` を名指ししても実行されないので、子の権限は渡した tool の一覧で決まる。子の一覧には `agent` が無いので、入れ子にならない。
* 成功したら、子の最後の assistant message の本文を tool 結果にする。20,000 文字を超えた分は切り、切ったことを書き添える。
* tool 結果の末尾に、子の round 数、tool 呼び出しの数、token 使用量（入力は cache を含む）、所要時間を一行で付ける。失敗した場合も付ける。親のモデルと利用者が、委任にかかった費用を知れるようにするため。数は子の Activity event から数えるので、途中で止まった実行も測れる。
* 失敗（provider のエラー、round の上限、取り消し）は `is_error` の tool 結果にする。`worker` の失敗には、それまでの変更が残っていることを書き添える。親は続けて動ける。取り消しは、子の結果を返したあと親の tool loop も止める。

### 表示と記録

* 子が承認を求めるとき、承認の要求に子の種類と `description` を付ける（`AgentProgress::ApprovalRequested.requested_by`）。TUI は承認の選択肢の上の要約と、要求の詳細の冒頭にそれを出す。表示のためだけの情報で、承認は従来どおり要求の ID で照合する。
* 子の Activity event は、`ActivityEvent.agent` にそれを起動した `agent` 呼び出しの call ID を入れて親に渡す。親の event には付けない。JSONL の記録にも同じ形で残り、この項目の無い古い記録はそのまま読める。
* TUI は `agent` の付いた項目を `│ ` の罫線で字下げして出す。子の round 番号は 1 から数え直すので、項目の照合には `turn_id` と `round` に `agent` を加える。
* 子の本文の stream、thinking の stream、生の provider event、model request の開始、会話欄に足す tool 呼び出しの項目は親に渡さない。会話欄に親の発言や呼び出しとして混ざり、再起動後の会話欄と食い違うため。会話欄には `agent` 呼び出しとその結果だけが出て、子の tool 呼び出しは Activity 欄に出る。子の round では、生の event 表示（`r`）を出さず、理由を示す。
* 子の message は保存しない。親の会話には、`agent` 呼び出しの開始（`tool_started`）と結果だけが残る。子の実行中に止まった場合は、他の tool と同じく結果不明として扱い、再実行しない。子が呼んだ tool は、Activity の記録に残る。（ADR 10 で、子の message を保存し、再開できるようにした。）
* 子の token 使用量は、turn の出力 token の合計に含める。親の自動 compaction の判断には使わない。

### 段階

1. 同期の `agent` tool と、組み込みの `explorer` と `worker`（この ADR）。
2. 利用者が定義する種類。名前、説明、tool の一覧、system prompt、モデルを設定に書く。tool の一覧は、組み込みの種類と同じく渡した tool の照合で強制する。形式と置き場所は ADR 9 で決めた。
3. 一つの応答にある複数の `explorer` 呼び出しを並行に実行する。読み取りだけの種類に限れば、承認と編集の競合は起きない。ADR 10 で実装した。

### Consequences

* Good, because 広い調査の途中経過が親の文脈に入らず、報告だけが残る。
* Good, because 子は承認、sandbox、取り消しを親と共有し、tool の一覧の外を呼んでも実行されない。
* Good, because 今の tool loop を再帰的に呼ぶだけで済み、新しい永続化の形式が要らない。
* Bad, because 子が動く間、親は待つ。複数の調査を同時に進められない。
* Bad, because 子の message を保存しないので、再起動後に子の途中経過を見返すには Activity の記録しかない（ADR 10 で解消）。
* Bad, because 子の報告の質はモデルに依存する。親のモデルは報告を鵜呑みにしうる。tool の説明で、報告を確かめてから利用者に伝えるよう求めている。
* Neutral, because 子は親と同じモデルを使う。安いモデルに調査を任せる方法は ADR 9 と ADR 10 で足した。

### Confirmation

core の単体テストで、`explorer` が空の文脈と自分の system prompt で始まり読み取り用の tool だけを受け取ること、親には報告だけが返り子の tool 呼び出しが会話に残らないこと、実行中の会話欄に出る tool の項目が再起動後に会話から組み直すものと一致すること、子の Activity に call ID が付くこと、一覧の外の tool と入れ子の `agent` が拒否されファイルが変わらないこと、`worker` が `agent` 以外のすべての tool を受け取ること、子の失敗が `is_error` の結果になり親が続くこと、不正な入力がモデルの呼び出し前に拒否されることを、`ScriptedProvider` で確認する。TUI のテストで、子の event が親の同じ番号の round の項目に混ざらないこと、子の round で生の event 表示を断ること、罫線の字下げを確認する。

## Pros and Cons of the Options

### 同期の `agent` tool

* Good, because 一度に一件の承認と、順に実行する tool loop をそのまま使える。
* Good, because 親のモデルから見れば普通の tool で、結果が同じ round で返る。
* Bad, because 子の実行中は親が止まる。

### 非同期の子エージェントと管理用の tool 群

* Good, because 親と複数の子が同時に進み、長い作業を並べられる。
* Good, because 子に追加の指示を送れ、途中で止められる。
* Bad, because 承認の要求が複数の子から同時に来うる。今の `TaskControl` は一度に一件しか持たない。
* Bad, because 同じファイルへの同時の編集と、それぞれの取り消し履歴の整合を新たに保証する必要がある。
* Bad, because 子の状態を turn をまたいで持ち、再起動後に復元する仕組みが要る。

### 利用者が定義する種類を最初から加える

* Good, because 用途ごとの system prompt と tool の組み合わせを作れる。
* Bad, because 設定の形式、検証、読み込み場所を先に決める必要がある。`.socium/` は sandbox の中のコマンドから隠すエージェントの状態の場所なので、定義の置き場所を別に考える必要がある。
* Bad, because 組み込みの二種で足りるかを確かめる前に、形式を固定することになる。

## More Information

Claude Code の仕組みは公開文書（https://docs.anthropic.com/en/docs/claude-code/sub-agents ）、Codex CLI の仕組みは openai/codex の `codex-rs/core/src/tools/spec_plan.rs` と multi-agent の handler を、2026-11-14 に確認した。関連: ADR 2（Activity 欄の構造化 event）、ADR 4（Activity の記録）。
