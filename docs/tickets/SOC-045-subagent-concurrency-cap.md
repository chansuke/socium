# SOC-045: 同時に動くサブエージェントの数に上限を設ける

GitHub: なし (2026-11-19 の公開前レビューで発見)

## 問題
1 つの応答に続けて並んだ読み取り専用のサブエージェントは、数に関係なくすべて同時に始まっていた。リポジトリに書かれた指示や壊れたモデル出力が数十件の `agent` 呼び出しを並べると、その数だけモデルへの接続、メモリ、レート制限を同時に使う。

## 対応・受け入れ条件
- 同時に動くサブエージェントを `MAX_CONCURRENT_SUBAGENTS` (4) までにする。残りは次のまとまりで動き、どの呼び出しも捨てない。
- 呼び出しの総数はこれまでどおり、モデルの出力の長さと `MAX_TOOL_ROUNDS` で決まる。費用の上限は設けない。

## 完了記録

2026-11-19 完了。

- 試験: `crates/core/src/agent/subagent/mod.rs` の `a_long_run_of_subagents_runs_a_few_at_a_time_and_keeps_every_result` で、1 つの応答の 6 件の explorer が同時には 4 件までしか動かず、6 件の結果が呼び出し順にモデルへ返ることを確かめる。上限を外すと 6 件が同時に動き、この試験は落ちる。`turn.rs` の `only_consecutive_new_read_only_subagents_are_batched` は、まとまりの長さが上限で切れることを確かめる。
