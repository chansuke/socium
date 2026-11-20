---
number: 24
title: 追加の拡張は実装済みの範囲に留めskills-session-fork-pluginは入れない
status: proposed
date: 2026-11-20
---

# 追加の拡張は実装済みの範囲に留めskills-session-fork-pluginは入れない

## Context and Problem Statement

2026-11-11 のレビューでは、MCP、skills の高度化、plan、subagents、Web UI、session の fork、読み取り専用の並列実行、plugin の枠組みと hot reload を、まとめて入れる候補に挙げた。まとめて入れると、基盤の修正より先に、保守と権限と lifecycle の負担が増える。候補ごとに、今どこまであるか、評価のタスクでどれだけ使われるか、既存の機能で代わりになるかを見て、採否を決める。

## Decision Drivers

* 評価のタスクか利用者の作業で、必要だと示せる。
* 権限の境界、取り消し、再開の扱いが、入れる前に決まっている。
* 既存の機能で代わりになるなら、新しい仕組みを足さない。

## Considered Options

* 候補ごとに、今の範囲に留めるか、入れるか、見送るかを決める
* 挙げた候補をまとめて入れる
* plugin の枠組みを入れ、各候補をその上に載せる

## Decision Outcome

Chosen option: 「候補ごとに決める」。まとめて入れる根拠も、枠組みを先に作る根拠も、測った作業からは出てこない。

使用の回数は、claude-haiku-5-5 で 39 タスクを各 3 回、prompt cache の有無で 2 組実行した 234 回の記録 (chansuke/socium#26) で数えた。ツール呼び出しは全部で 1,758 回だった。

| 候補 | 決定 | 根拠 |
|---|---|---|
| plan | 今の範囲に留める | `update_plan` と Timeline の表示は ADR 18 で入っている。評価では 8 回使われた。 |
| subagents | 今の範囲に留める | `agent` tool、利用者の定義する種類、記録と再開は ADR 8〜10 で入っている。評価では使われなかった。 |
| 読み取り専用の並列実行 | 今の範囲に留める | ADR 10 で、続けて並ぶ読み取り専用の子だけを同時に動かし、承認を求めず、結果を呼び出しの順に入れる規約とともに入っている。書き込む子の並列は、編集の順序と取り消しの履歴を保つ仕組みがないので入れない。 |
| Web UI | 今の範囲に留める | gateway と web と mobile の client が入っている (`docs/clients.md`)。評価の対象外。 |
| MCP | この ADR では決めない | 別の作業で、MCP の PR と、選んだ server を読み取り専用の sandbox で動かす ADR の案を進めている。採否はそちらで決める。 |
| skills の高度化 | 見送る | 該当する機能はない。プロジェクトの指示は `AGENTS.md` の読み込み、作業の型は利用者の定義する subagent (ADR 9) で代わりになる。評価に需要を示すタスクはない。 |
| session の fork | 見送る | `/rewind` で前のプロンプトに戻れ、`-c` と `-r` で保存した会話を続けられる。分岐を並べて持つ需要は、評価にも利用者の作業にも見えていない。 |
| plugin の枠組みと hot reload | 見送る | 載せる候補がなく、他の道具が持っているという理由だけでは入れない。入れると、読み込んだ code の権限と、実行中の差し替えでの取り消しと再開を新たに決める必要がある。 |

見送った候補を入れ直すときは、需要を示す評価のタスクを `evals/tasks` に足し、受け入れ条件、権限の境界、取り消しと再開、運用の負担を定めた実装のチケットを先に切る。

### Consequences

* Good, because 権限、session、取り消しの扱いを変える新しい仕組みが増えない。
* Good, because 見送った候補を入れ直す条件が決まっていて、提案だけでは入らない。
* Bad, because 評価のタスクは小さい Python のリポジトリが中心で、大きなリポジトリや長い作業での需要は測っていない。
* Bad, because skills や plugin に慣れた利用者は、同じことを `AGENTS.md` と subagent の定義で書き直す必要がある。

### Confirmation

この ADR は採否の記録で、code は変えない。見直すときは、`evals/run.py` で候補が効く作業のタスクを足し、`compare.py` で入れる前と後を比べる。

## Pros and Cons of the Options

### 候補ごとに決める

* Good, because 入っているものは保ち、需要のないものに保守の負担を払わない。
* Bad, because 候補ごとに根拠を集める手間がかかる。

### まとめて入れる

* Good, because 他の coding agent と機能の一覧が揃う。
* Bad, because 権限と lifecycle の負担が一度に増え、どれが効いたかも測れない。

### plugin の枠組みを先に入れる

* Good, because 後の拡張を本体を変えずに足せる。
* Bad, because 載せる候補がないまま、読み込む code の権限と差し替えの扱いを抱える。

## More Information

元のチケットは SOC-036 (chansuke/socium#41)。
